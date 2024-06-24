mod auth;
mod db;
mod poll;
mod room;

use poll::{cleanup, ident, poll};
use worker::{
    event, Context, Cors, Env, Headers, Method, Request, Response, Result, ScheduleContext,
    ScheduledEvent,
};

async fn handle(req: Request, env: Env) -> Result<Response> {
    if !matches!(req.method(), Method::Post) {
        return Response::error("Method Not Allowed", 405);
    }

    let path = req.path();
    if path == "/ident" {
        return ident(env).await;
    } else if path == "/poll" {
        return poll(req, env).await;
    }

    Response::error("Page Not Found", 404)
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let cors = Cors::new()
        .with_max_age(86400)
        .with_credentials(true)
        .with_methods([Method::Options, Method::Post])
        .with_origins(["*"])
        .with_allowed_headers(["Authorization", "*"]);

    if matches!(req.method(), Method::Options) {
        let mut headers = Headers::new();
        headers.set("Allow", "OPTIONS, POST")?;
        return Response::empty()?.with_headers(headers).with_cors(&cors);
    }
    handle(req, env).await?.with_cors(&cors)
}

#[event(scheduled)]
async fn do_cleanup(_evt: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let bucket = env.bucket("rtc").expect("missing R2 bucket");
    cleanup(bucket).await;
}
