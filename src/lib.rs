use anyhow::Result;
use spin_sdk::{
    http::{Request, Response},
    http_component,
};

use libsql_client::{args, spin::Client, ResultSet, Statement};

// Take a query result and render it into a HTML table
fn result_to_html_table(result_set: ResultSet) -> Result<String> {
    let mut html = "<table style=\"border: 1px solid\">".to_string();
    for column in &result_set.columns {
        html += &format!("<th style=\"border: 1px solid\">{column}</th>");
    }
    for row in result_set.rows {
        html += "<tr style=\"border: 1px solid\">";
        for value in row.values {
            match value {
                libsql_client::Value::Text { value } => html += &format!("<td>{value}</td>"),
                libsql_client::Value::Integer { value } => html += &format!("<td>{value}</td>"),
                libsql_client::Value::Null => html += "<td>(null)</td>",
                _ => html += "<td>(unexpected value type!)</td>",
            }
        }
        html += "</tr>";
    }
    html += "</table>";
    Ok(html)
}

// Create a javascript canvas which loads a map of visited airports
fn create_map_canvas(result_set: ResultSet) -> Result<String> {
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
      canvas.parent('map');
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

    for row in result_set.rows {
        println!(
            "{} {} {}",
            row.value_map["lat"], row.value_map["long"], row.value_map["airport"]
        );
        canvas += &format!(
            "point = myMap.latLngToPixel({}, {});\nellipse(point.x, point.y, 10, 10);\ntext({}, point.x, point.y);\n",
            row.value_map["lat"], row.value_map["long"], row.value_map["airport"]
        );
    }

    canvas += "}</script>";
    Ok(canvas)
}

// Serve a request to load the page
fn serve(db: Client, client_addr: &str) -> Result<String> {
    // Recreate the tables if they do not exist yet
    db.execute("CREATE TABLE IF NOT EXISTS counter(country TEXT, city TEXT, value, PRIMARY KEY(country, city)) WITHOUT ROWID")?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS coordinates(lat INT, long INT, airport TEXT, PRIMARY KEY (lat, long))",
    )?;

    let req = http::Request::builder().uri(format!(
        "http://ip-api.com/json/{client_addr}?fields=country,city,lat,lon"
    ));
    let geo = spin_sdk::outbound_http::send_request(req.body(None)?)?;
    let geo = geo.into_body().expect("Received empty geolocation data");
    let geo: serde_json::Value = serde_json::from_str(std::str::from_utf8(&geo)?)?;

    let city = geo["city"].as_str().unwrap_or("Secret Turso HQ");
    let country = geo["country"].as_str().unwrap_or("[undisclosed]");
    let airport = city;
    let latitude = geo["lat"].as_f64().unwrap_or_default();
    let longitude = geo["lon"].as_f64().unwrap_or_default();

    db.batch([
        Statement::with_args(
            "INSERT OR IGNORE INTO counter VALUES (?, ?, 0)",
            &[country, city],
        ),
        Statement::with_args(
            "UPDATE counter SET value = value + 1 WHERE country = ? AND city = ?",
            &[country, city],
        ),
        Statement::with_args(
            "INSERT OR IGNORE INTO coordinates VALUES (?, ?, ?)",
            args!(latitude, longitude, airport),
        ),
    ])?;

    let counter_response = db.execute("SELECT * FROM counter")?;
    let scoreboard = result_to_html_table(counter_response)?;

    let coords = db.execute("SELECT airport, lat, long FROM coordinates")?;
    let canvas = create_map_canvas(coords)?;
    let html = format!(
        r#"
        <h1>Spin + Turso demo</h1>
        <h3>Each request bumps a counter at detected location</h3>
        {canvas}
        <div style="display:flex">
            <div style="margin-right: 5px"> <h2>Scoreboard:<h2> {scoreboard}</div>
            <div id="map"></div>
        </div>
        <p>Database powered by <a href=\"https://chiselstrike.com/\">Turso</a><p>
        <footer>Map data from OpenStreetMap (https://tile.osm.org/)<br />geolocation from http://ip-api.com</footer>"#
    );
    Ok(html)
}

/// A simple Spin HTTP component.
#[http_component]
fn handle_country_counter_spin(req: Request) -> Result<Response> {
    println!("{:?}", req.uri());
    println!("{:?}", req.headers());
    println!(
        "{:?}",
        req.extensions().get::<Option<std::net::SocketAddr>>()
    );

    let client_addr = req
        .headers()
        .get("spin-client-addr")
        .map(|v| {
            let addr = v.to_str().unwrap();
            match addr.find(':') {
                Some(i) => &addr[..i],
                None => addr,
            }
            .to_string()
        })
        .unwrap_or_else(|| String::from("127.0.0.1"));

    let db = libsql_client::spin::Client::from_url(
        "https://psarna:H35VRkK9j14627Cy@spin-psarna.turso.io",
    )
    .unwrap();

    let html = match serve(db, &client_addr) {
        Ok(html) => html,
        Err(e) => format!("{e}"),
    };

    Ok(http::Response::builder()
        .status(200)
        .body(Some(html.into()))?)
}
