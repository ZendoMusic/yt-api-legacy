use std::io::Write;
use std::task::{Context, Poll};
use chrono::Local;
use colored::*;
use actix_web::{
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures_util::future::LocalBoxFuture;
use std::future::{ready, Ready};

pub fn init_logger() {
    std::env::set_var("RUST_LOG", "info");
    env_logger::builder()
        .format_timestamp(None)
        .format_module_path(false)
        .format_target(false)
        .filter_level(log::LevelFilter::Info)
        .format(format_log)
        .init();
}

fn format_log(
    buf: &mut env_logger::fmt::Formatter,
    record: &log::Record,
) -> std::io::Result<()> {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    
    let level_str = match record.level() {
        log::Level::Error => "[ERROR]".red(),
        log::Level::Warn => "[WARN]".yellow(),
        log::Level::Info => "[INFO]".green(),
        log::Level::Debug => "[DEBUG]".blue(),
        log::Level::Trace => "[TRACE]".purple(),
    };
    
    writeln!(
        buf,
        "{} {} {}",
        timestamp.dimmed(),
        level_str,
        record.args()
    )
}

pub use log::info;

#[derive(Default)]
pub struct SelectiveLogger;

impl<S, B> Transform<S, ServiceRequest> for SelectiveLogger
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = SelectiveLoggerMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(SelectiveLoggerMiddleware { service }))
    }
}

pub struct SelectiveLoggerMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for SelectiveLoggerMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let fut = self.service.call(req);
        
        Box::pin(async move {
            let res = fut.await?;
            let status = res.status();
            
            if status.as_u16() != 200 {
                info!("{} {} - {}", 
                    status.as_u16(), 
                    status.canonical_reason().unwrap_or("Unknown"),
                    res.request().path()
                );
            }
            
            Ok(res)
        })
    }
}