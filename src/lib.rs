use anyhow::Result;
use spin_sdk::{
    http::{Request, Response},
    http_component,
};

use libsql_client::{Connection, QueryResult, Statement, Value};
use rand::seq::SliceRandom;
use std::task::Poll;

// Take a query result and render it into a HTML table
fn result_to_html_table(result: QueryResult) -> String {
    let mut html = "<table style=\"border: 1px solid\">".to_string();
    match result {
        QueryResult::Error((msg, _)) => return format!("Error: {msg}"),
        QueryResult::Success((result, _)) => {
            for column in &result.columns {
                html += &format!("<th style=\"border: 1px solid\">{column}</th>");
            }
            for row in result.rows {
                html += "<tr style=\"border: 1px solid\">";
                for column in &result.columns {
                    html += &format!("<td>{}</td>", row.cells[column]);
                }
                html += "</tr>";
            }
        }
    };
    html += "</table>";
    html
}

// Create a javascript canvas which loads a map of visited airports
fn create_map_canvas(result: QueryResult) -> String {
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

    match result {
        QueryResult::Error((msg, _)) => return format!("Error: {msg}"),
        QueryResult::Success((result, _)) => {
            for row in result.rows {
                canvas += &format!(
                    "point = myMap.latLngToPixel({}, {});\nellipse(point.x, point.y, 10, 10);\ntext({}, point.x, point.y);\n",
                    row.cells["lat"], row.cells["long"], row.cells["airport"]
                );
            }
        }
    };
    canvas += "}</script>";
    canvas
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
fn serve(db: impl Connection) -> String {
    let waker = dummy_waker();
    let mut ctx = std::task::Context::from_waker(&waker);

    // Recreate the tables if they do not exist yet
    db.execute("CREATE TABLE IF NOT EXISTS counter(country TEXT, city TEXT, value, PRIMARY KEY(country, city)) WITHOUT ROWID")
    .as_mut().poll(&mut ctx).is_ready();
    db.execute(
        "CREATE TABLE IF NOT EXISTS coordinates(lat INT, long INT, airport TEXT, PRIMARY KEY (lat, long))",
    ).as_mut().poll(&mut ctx).is_ready();

    // For demo purposes, let's pick a pseudorandom location
    const FAKE_LOCATIONS: &[(&str, &str, &str, f64, f64)] = &[
        ("WAW", "PL", "Warsaw", 52.22959, 21.0067),
        ("EWR", "US", "Newark", 42.99259, -81.3321),
        ("HAM", "DE", "Hamburg", 50.118801, 7.684300),
        ("HEL", "FI", "Helsinki", 60.3183, 24.9497),
        ("NSW", "AU", "Sydney", -33.9500, 151.1819),
    ];

    let (airport, country, city, latitude, longitude) =
        *FAKE_LOCATIONS.choose(&mut rand::thread_rng()).unwrap();

    db.transaction([
        Statement::with_params("INSERT INTO counter VALUES (?, ?, 0)", &[country, city]),
        Statement::with_params(
            "UPDATE counter SET value = value + 1 WHERE country = ? AND city = ?",
            &[country, city],
        ),
        Statement::with_params(
            "INSERT INTO coordinates VALUES (?, ?, ?)",
            &[
                Value::Float(latitude),
                Value::Float(longitude),
                airport.into(),
            ],
        ),
    ])
    .as_mut()
    .poll(&mut ctx)
    .is_ready();

    let counter_response = match db.execute("SELECT * FROM counter").as_mut().poll(&mut ctx) {
        Poll::Ready(Ok(resp)) => resp,
        Poll::Ready(Err(e)) => return format!("Error: {e}"),
        Poll::Pending => return "Unexpected incomplete async event".into(),
    };
    let scoreboard = result_to_html_table(counter_response);

    let coords = match db
        .execute("SELECT airport, lat, long FROM coordinates")
        .as_mut()
        .poll(&mut ctx)
    {
        Poll::Ready(Ok(coords)) => coords,
        Poll::Ready(Err(e)) => return format!("Error: {e}"),
        Poll::Pending => return "Unexpected incomplete async event".into(),
    };
    let canvas = create_map_canvas(coords);
    let html = format!("{canvas} Database powered by <a href=\"https://chiselstrike.com/\">Turso</a>. <br /> Scoreboard: <br /> {scoreboard} <footer>Map data from OpenStreetMap (https://tile.osm.org/)</footer>");
    html
}

/// A simple Spin HTTP component.
#[http_component]
fn handle_country_counter_spin(req: Request) -> Result<Response> {
    println!("{:?}", req.headers());

    let db = libsql_client::spin::Connection::connect(
        "https://spin-psarna.turso.io",
        "psarna",
        "48EkN63vyf105ut2",
    );

    let html = serve(db);

    Ok(http::Response::builder()
        .status(200)
        .body(Some(html.into()))?)
}
