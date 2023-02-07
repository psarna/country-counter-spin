use anyhow::Result;
use spin_sdk::{
    http::{Request, Response},
    http_component,
};

use libsql_client::{spin::Connection, CellValue, QueryResult, Statement};

// Take a query result and render it into a HTML table
fn result_to_html_table(result: QueryResult) -> String {
    let mut html = "<table style=\"border: 1px solid\">".to_string();
    match result {
        QueryResult::Error((msg, _)) => return format!("Error: {}", msg),
        QueryResult::Success((result, _)) => {
            for column in &result.columns {
                html += &format!("<th style=\"border: 1px solid\">{}</th>", column);
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
        QueryResult::Error((msg, _)) => return format!("Error: {}", msg),
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

// Serve a request to load the page
fn serve(db: Connection) -> String {
    // Recreate the tables if they do not exist yet
    db.execute("CREATE TABLE IF NOT EXISTS counter(country TEXT, city TEXT, value, PRIMARY KEY(country, city)) WITHOUT ROWID")
    .ok();
    db.execute(
        "CREATE TABLE IF NOT EXISTS coordinates(lat INT, long INT, airport TEXT, PRIMARY KEY (lat, long))",
    )
    .ok();

    // Fake data
    let airport = "madeupfortesting";
    let country = "MadeUpForTesting";
    let city = "MadeUpForTesting";
    let coordinates = (0., 0.);
    db.transaction([
        Statement::with_params("INSERT INTO counter VALUES (?, ?, 0)", &[country, city]),
        Statement::with_params(
            "UPDATE counter SET value = value + 1 WHERE country = ? AND city = ?",
            &[country, city],
        ),
        Statement::with_params(
            "INSERT INTO coordinates VALUES (?, ?, ?)",
            &[
                CellValue::Float(coordinates.0 as f64),
                CellValue::Float(coordinates.1 as f64),
                airport.into(),
            ],
        ),
    ])
    .ok();

    let counter_response = match db.execute("SELECT * FROM counter") {
        Ok(resp) => resp,
        Err(e) => return format!("Error: {e}"),
    };
    let scoreboard = result_to_html_table(counter_response);

    let coords = match db.execute("SELECT airport, lat, long FROM coordinates") {
        Ok(coords) => coords,
        Err(e) => return format!("Error: {e}"),
    };
    let canvas = create_map_canvas(coords);
    let html = format!("{} Scoreboard: <br /> {}", canvas, scoreboard);
    html
}

/// A simple Spin HTTP component.
#[http_component]
fn handle_country_counter_spin(req: Request) -> Result<Response> {
    println!("{:?}", req.headers());

    let db = Connection::connect("https://spin-psarna.turso.io", "psarna", "48EkN63vyf105ut2");

    let html = serve(db);

    Ok(http::Response::builder()
        .status(200)
        .body(Some(html.into()))?)
}
