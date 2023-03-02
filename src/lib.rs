use anyhow::Result;
use spin_sdk::{
    http::{Request, Response},
    http_component,
};

use libsql_client::{params, Connection, QueryResult, ResultSet, Statement};
use std::task::Poll;

// Take a query result and render it into a HTML table
fn result_to_html_table(result: QueryResult) -> Result<String> {
    let mut html = "<table style=\"border: 1px solid\">".to_string();
    let ResultSet { columns, rows } = result.into_result_set()?;
    for column in &columns {
        html += &format!("<th style=\"border: 1px solid\">{column}</th>");
    }
    for row in rows {
        html += "<tr style=\"border: 1px solid\">";
        for column in &columns {
            html += &format!("<td>{}</td>", row.cells[column]);
        }
        html += "</tr>";
    }
    html += "</table>";
    Ok(html)
}

// Create a javascript canvas which loads a map of visited airports
fn create_map_canvas(result: QueryResult) -> Result<String> {
    let mut canvas = r#"
  <script src="https://cdnjs.cloudflare.com/ajax/libs/p5.js/0.5.16/p5.min.js" type="text/javascript"></script>
  <script src="https://unpkg.com/mappa-mundi/dist/mappa.js" type="text/javascript"></script>
    <script>
    let myMap;
    let canvas;
    const mappa = new Mappa('Leaflet');
    const options = {
      lat: 0,
      lng: 0,
      zoom: 1,
      style: "http://{s}.tile.osm.org/{z}/{x}/{y}.png"
    }
    function setup(){
      canvas = createCanvas(640,480);
      myMap = mappa.tileMap(options); 
      myMap.overlay(canvas) 
    
      fill(200, 100, 100);
      myMap.onChange(drawPoint);
    }
    function draw(){
    }
    function drawPoint(){
      clear();
      let point;"#.to_owned();

    let ResultSet { columns: _, rows } = result.into_result_set()?;
    for row in rows {
        canvas += &format!(
            "point = myMap.latLngToPixel({}, {});\nellipse(point.x, point.y, 10, 10);\ntext({}, point.x, point.y);\n",
            row.cells["lat"], row.cells["long"], row.cells["airport"]
        );
    }

    canvas += "}</script>";
    Ok(canvas)
}

// Helper function to be able to poll async functions in sync code
fn dummy_waker() -> std::task::Waker {
    const VTABLE: std::task::RawWakerVTable = std::task::RawWakerVTable::new(
        |data: *const ()| std::task::RawWaker::new(data, &VTABLE),
        |_data: *const ()| (),
        |_data: *const ()| (),
        |_data: *const ()| (),
    );
    const RAW: std::task::RawWaker = std::task::RawWaker::new(
        (&VTABLE as *const std::task::RawWakerVTable).cast(),
        &VTABLE,
    );
    unsafe { std::task::Waker::from_raw(RAW) }
}

// Serve a request to load the page
fn serve(db: impl Connection) -> Result<String> {
    let waker = dummy_waker();
    let mut ctx = std::task::Context::from_waker(&waker);

    // Recreate the tables if they do not exist yet
    db.execute("CREATE TABLE IF NOT EXISTS counter(country TEXT, city TEXT, value, PRIMARY KEY(country, city)) WITHOUT ROWID")
    .as_mut().poll(&mut ctx).is_ready();
    db.execute(
        "CREATE TABLE IF NOT EXISTS coordinates(lat INT, long INT, airport TEXT, PRIMARY KEY (lat, long))",
    ).as_mut().poll(&mut ctx).is_ready();

    let req = http::Request::builder().uri("http://www.geoplugin.net/json.gp");
    let geo = spin_sdk::outbound_http::send_request(req.body(None)?)?;
    let geo = geo.into_body().expect("Received empty geolocation data");
    let geo: serde_json::Value = serde_json::from_str(std::str::from_utf8(&geo)?)?;

    let airport = geo["geoplugin_city"].as_str().unwrap_or_default();
    let country = geo["geoplugin_countryName"].as_str().unwrap_or_default();
    let city = geo["geoplugin_city"].as_str().unwrap_or_default();
    let latitude = geo["geoplugin_latitude"]
        .as_str()
        .unwrap_or_default()
        .parse::<f64>()?;
    let longitude = geo["geoplugin_longitude"]
        .as_str()
        .unwrap_or_default()
        .parse::<f64>()?;

    db.transaction([
        Statement::with_params("INSERT INTO counter VALUES (?, ?, 0)", &[country, city]),
        Statement::with_params(
            "UPDATE counter SET value = value + 1 WHERE country = ? AND city = ?",
            &[country, city],
        ),
        Statement::with_params(
            "INSERT INTO coordinates VALUES (?, ?, ?)",
            params!(latitude, longitude, airport),
        ),
    ])
    .as_mut()
    .poll(&mut ctx)
    .is_ready();

    let counter_response = match db.execute("SELECT * FROM counter").as_mut().poll(&mut ctx) {
        Poll::Ready(Ok(resp)) => resp,
        Poll::Ready(Err(e)) => anyhow::bail!("Error: {e}"),
        Poll::Pending => anyhow::bail!("Unexpected incomplete async event"),
    };
    let scoreboard = result_to_html_table(counter_response)?;

    let coords = match db
        .execute("SELECT airport, lat, long FROM coordinates")
        .as_mut()
        .poll(&mut ctx)
    {
        Poll::Ready(Ok(coords)) => coords,
        Poll::Ready(Err(e)) => anyhow::bail!("Error: {e}"),
        Poll::Pending => anyhow::bail!("Unexpected incomplete async event"),
    };
    let canvas = create_map_canvas(coords)?;
    let html = format!("{canvas} Database powered by <a href=\"https://chiselstrike.com/\">Turso</a>. <br /> Scoreboard: <br /> {scoreboard} <footer>Map data from OpenStreetMap (https://tile.osm.org/)</footer>");
    Ok(html)
}

/// A simple Spin HTTP component.
#[http_component]
fn handle_country_counter_spin(req: Request) -> Result<Response> {
    println!("{:?}", req.uri());
    println!("{:?}", req.headers());

    let db = libsql_client::spin::Connection::connect(
        "https://spin-psarna.turso.io",
        "psarna",
        "9J41z0x85j7Qbvn2",
    );

    let html = match serve(db) {
        Ok(html) => html,
        Err(e) => format!("{e}"),
    };

    Ok(http::Response::builder()
        .status(200)
        .body(Some(html.into()))?)
}
