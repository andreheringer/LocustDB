use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use actix_web::web::Data;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tera::{Context, Tera};

use crate::ingest::raw_val::RawVal;
use crate::LocustDB;
use crate::Value;

lazy_static! {
    pub static ref TEMPLATES: Tera = {
        let mut tera = match Tera::new("templates/**/*") {
            Ok(t) => t,
            Err(e) => {
                println!("Parsing error(s): {}", e);
                ::std::process::exit(1);
            }
        };
        tera.autoescape_on(vec!["html", ".sql"]);
        // tera.register_filter("do_nothing", do_nothing_filter);
        tera
    };
}

#[derive(Serialize, Deserialize, Debug)]
struct DataBatch {
    pub table: String,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
}

#[derive(Clone)]
struct AppState {
    db: Arc<LocustDB>,
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryRequest {
    query: String,
}

#[get("/")]
async fn index(data: web::Data<AppState>) -> impl Responder {
    let mut context = Context::new();
    let mut ts: Vec<String> = data
        .db
        .table_stats()
        .await
        .unwrap()
        .into_iter()
        .map(|ts| ts.name)
        .collect::<Vec<_>>();
    ts.sort();
    context.insert("tables", &ts);
    let body = TEMPLATES.render("index.html", &context).unwrap();
    HttpResponse::Ok()
        .content_type("text/html; charset=utf8")
        .body(body)
}

#[get("/plot")]
async fn plot(_data: web::Data<AppState>) -> impl Responder {
    let context = Context::new();
    let body = TEMPLATES.render("plot.html", &context).unwrap();
    HttpResponse::Ok()
        .content_type("text/html; charset=utf8")
        .body(body)
}

#[get("/table/{tablename}")]
async fn table_handler(
    path: web::Path<String>,
    data: web::Data<AppState>,
) -> impl Responder {
    let cols = data
        .db
        .run_query(
            &format!("SELECT * FROM {} LIMIT 0", path.as_str()),
            false,
            vec![],
        )
        .await
        .unwrap()
        .unwrap()
        .colnames;

    let mut context = Context::new();
    context.insert("columns", &cols.join(", "));
    context.insert("table", path.as_str());
    let body = TEMPLATES.render("table.html", &context).unwrap();

    HttpResponse::Ok()
        .content_type("text/html; charset=utf8")
        .body(body)
}

#[get("/tables")]
async fn tables(data: web::Data<AppState>) -> impl Responder {
    println!("Requesting table stats");
    let stats = data.db.table_stats().await.unwrap();

    let mut body = String::new();
    for table in stats {
        writeln!(body, "{}", table.name).unwrap();
        writeln!(body, "  Rows: {}", table.rows).unwrap();
        writeln!(body, "  Batches: {}", table.batches).unwrap();
        writeln!(body, "  Batches bytes: {}", table.batches_bytes).unwrap();
        writeln!(body, "  Buffer length: {}", table.buffer_length).unwrap();
        writeln!(body, "  Buffer bytes: {}", table.buffer_bytes).unwrap();
        //writeln!(body, "  Size per column: {}", table.size_per_column).unwrap();
    }
    HttpResponse::Ok().body(body)
}

#[post("/echo")]
async fn echo(req_body: String) -> impl Responder {
    HttpResponse::Ok().body(req_body)
}

#[get("/query_data")]
async fn query_data(_data: web::Data<AppState>) -> impl Responder {
    let response = json!({
        "cols": ["time", "cpu"],
        "series": [
            [1640025197013.0, 1640025198013.0, 1640025199013.0, 1640025200013.0, 1640025201013.0, 1640025202113.0, 1640025203113.0, 1640025204113.0, 1640025205113.0],
            [0.3, 0.4, 0.5, 0.2, 0.1, 0.3, 0.4, 0.5, 0.2]
        ]
    });
    HttpResponse::Ok().json(response)
}

#[post("/query")]
async fn query(data: web::Data<AppState>, req_body: web::Json<QueryRequest>) -> impl Responder {
    log::info!("Query: {:?}", req_body);
    let result = data
        .db
        .run_query(&req_body.query, false, vec![])
        .await
        .unwrap()
        .unwrap();

    let response = json!({
        "colnames": result.colnames,
        "rows": result.rows.iter().map(|row| row.iter().map(|val| match val {
            Value::Int(int) => json!(int),
            Value::Str(str) => json!(str),
            Value::Null => json!(null),
            Value::Float(float) => json!(float.0),
        }).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "stats": result.stats,
    });
    HttpResponse::Ok().json(response)
}

#[get("/query_cols")]
async fn query_cols(
    data: web::Data<AppState>,
    // req_body: web::Json<QueryRequest>,
) -> impl Responder {
    // log::info!("Query: {:?}", req_body);
    let result = data
        .db
        .run_query("SELECT timestamp, cpu * 100 AS cpu FROM test_metrics LIMIT 100000000", false, vec![])
        .await
        .unwrap()
        .unwrap();

    let mut cols: HashMap<String, Vec<serde_json::Value>> = HashMap::default();
    for col in &result.colnames {
        cols.insert(col.to_string(), vec![]);
    }
    for row in result.rows {
        for (val, colname) in row.iter().zip(result.colnames.iter()) {
            cols.get_mut(colname).unwrap().push(match val {
                Value::Int(int) => json!(int),
                Value::Str(str) => json!(str),
                Value::Null => json!(null),
                Value::Float(f) => json!(f.0),
            });
        }
    }
    let response = json!({
        "colnames": result.colnames,
        "cols": cols,
        "stats": result.stats,
    });
    HttpResponse::Ok().json(response)
}

// TODO: efficient endpoint
#[post("/insert")]
async fn insert(data: web::Data<AppState>, req_body: web::Json<DataBatch>) -> impl Responder {
    log::info!("Inserting! {:?}", req_body);
    let DataBatch { table, rows } = req_body.0;
    data.db
        .ingest(
            &table,
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(colname, val)| {
                            let val = match val {
                                serde_json::Value::Null => RawVal::Null,
                                serde_json::Value::Number(n) => {
                                    if n.is_i64() { 
                                        RawVal::Int(n.as_i64().unwrap())
                                    } else if n.is_f64() {
                                        RawVal::Float(OrderedFloat(n.as_f64().unwrap()))
                                    } else {
                                        panic!("Unsupported number {}", n)
                                    }
                                },
                                serde_json::Value::String(s) => RawVal::Str(s),
                                _ => panic!("Unsupported value: {:?}", val),
                            };
                            (colname, val)
                        })
                        .collect()
                })
                .collect(),
        )
        .await;
    HttpResponse::Ok().json(r#"{"status": "ok"}"#)
}

async fn manual_hello() -> impl Responder {
    HttpResponse::Ok().body("Hey there!")
}

pub async fn run(db: LocustDB) -> std::io::Result<()> {
    let db = Arc::new(db);
    HttpServer::new(move || {
        let app_state = AppState { db: db.clone() };
        App::new()
            .app_data(Data::new(app_state))
            .app_data(Data::new(web::PayloadConfig::new(100 * 1024 * 1024)))
            .service(index)
            .service(echo)
            .service(tables)
            .service(query)
            .service(table_handler)
            .service(insert)
            .service(query_data)
            .service(query_cols)
            .service(plot)
            .route("/hey", web::get().to(manual_hello))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
