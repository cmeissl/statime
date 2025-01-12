use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixStream},
};

use crate::config::Config;
use statime::{
    config::TimePropertiesDS,
    observability::{default::DefaultDS, ObservableInstanceState},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct ObservableState {
    pub program: ProgramData,
    pub instance: ObservableInstanceState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProgramData {
    pub version: String,
    pub build_commit: String,
    pub build_commit_date: String,
    pub uptime_seconds: f64,
}

impl ProgramData {
    pub fn with_uptime(uptime_seconds: f64) -> ProgramData {
        ProgramData {
            uptime_seconds,
            ..Default::default()
        }
    }
}

impl Default for ProgramData {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            build_commit: env!("STATIME_GIT_REV").to_owned(),
            build_commit_date: env!("STATIME_GIT_DATE").to_owned(),
            uptime_seconds: 0.0,
        }
    }
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// Configuration file to use
    #[clap(
        long = "config",
        short = 'c',
        default_value = "/etc/statime/statime.toml"
    )]
    config: PathBuf,
}

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Args::parse();
    let config = match Config::from_file(options.config.as_path()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    crate::setup_logger(config.loglevel)?;

    let observation_socket_path = match config.observability.observation_path {
        Some(path) => path,
        None => {
            eprintln!(
                "An observation socket path must be configured using the observation-path option \
                 in the [observability] section of the configuration"
            );
            std::process::exit(1);
        }
    };

    println!(
        "starting statime-metrics-exporter on {}",
        &config.observability.metrics_exporter_listen
    );

    let listener = TcpListener::bind(&config.observability.metrics_exporter_listen).await?;
    let mut buf = String::with_capacity(4 * 1024);

    loop {
        let (mut tcp_stream, _) = listener.accept().await?;

        buf.clear();
        match handler(&mut buf, &observation_socket_path).await {
            Ok(()) => {
                tcp_stream.write_all(buf.as_bytes()).await?;
            }
            Err(e) => {
                log::warn!("error: {e}");
                const ERROR_REPONSE: &str = concat!(
                    "HTTP/1.1 500 Internal Server Error\r\n",
                    "content-type: text/plain\r\n",
                    "content-length: 0\r\n\r\n",
                );

                tcp_stream.write_all(ERROR_REPONSE.as_bytes()).await?;
            }
        }
    }
}

fn format_response(buf: &mut String, state: &ObservableState) -> std::fmt::Result {
    let mut content = String::with_capacity(4 * 1024);
    format_state(&mut content, state)?;

    // headers
    buf.push_str("HTTP/1.1 200 OK\r\n");
    buf.push_str("content-type: text/plain\r\n");
    buf.write_fmt(format_args!("content-length: {}\r\n\r\n", content.len()))?;

    // actual content
    buf.write_str(&content)?;

    Ok(())
}

pub async fn read_json<'a, T>(
    stream: &mut UnixStream,
    buffer: &'a mut Vec<u8>,
) -> std::io::Result<T>
where
    T: serde::Deserialize<'a>,
{
    buffer.clear();

    let n = stream.read_buf(buffer).await?;
    buffer.truncate(n);
    serde_json::from_slice(buffer)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

async fn handler(buf: &mut String, observation_socket_path: &Path) -> std::io::Result<()> {
    let mut stream = tokio::net::UnixStream::connect(observation_socket_path).await?;
    let mut msg = Vec::with_capacity(16 * 1024);
    let observable_state: ObservableState = read_json(&mut stream, &mut msg).await?;

    format_response(buf, &observable_state)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "formatting error"))
}

struct Measurement<T> {
    labels: Vec<(&'static str, String)>,
    value: T,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Unit {
    Seconds,
}

impl Unit {
    fn as_str(&self) -> &str {
        "seconds"
    }
}

#[allow(dead_code)]
enum MetricType {
    Gauge,
    Counter,
}

impl MetricType {
    fn as_str(&self) -> &str {
        match self {
            MetricType::Gauge => "gauge",
            MetricType::Counter => "counter",
        }
    }
}

fn format_default_ds(w: &mut impl std::fmt::Write, default_ds: &DefaultDS) -> std::fmt::Result {
    let clock_identity = format!("{}", default_ds.clock_identity);

    format_metric(
        w,
        "number_ports",
        "The amount of ports assigned",
        MetricType::Gauge,
        None,
        vec![Measurement {
            labels: vec![("clock_identity", clock_identity.clone())],
            value: default_ds.number_ports,
        }],
    )?;

    format_metric(
        w,
        "quality_class",
        "The PTP clock class",
        MetricType::Gauge,
        None,
        vec![Measurement {
            labels: vec![("clock_identity", clock_identity.clone())],
            value: default_ds.clock_quality.clock_class,
        }],
    )?;

    format_metric(
        w,
        "quality_accuracy",
        "The quality of the clock",
        MetricType::Gauge,
        None,
        vec![Measurement {
            labels: vec![("clock_identity", clock_identity.clone())],
            value: default_ds.clock_quality.clock_accuracy.to_primitive(),
        }],
    )?;

    format_metric(
        w,
        "quality_offset_scaled_log_variance",
        "2-log of the variance (in seconds^2) of the clock when not synchronized",
        MetricType::Gauge,
        None,
        vec![Measurement {
            labels: vec![("clock_identity", clock_identity.clone())],
            value: default_ds.clock_quality.offset_scaled_log_variance,
        }],
    )?;

    Ok(())
}

pub fn format_time_properties_ds(
    w: &mut impl std::fmt::Write,
    time_properties_ds: &TimePropertiesDS,
) -> std::fmt::Result {
    format_metric(
        w,
        "current_utc_offset",
        "Current offset from UTC",
        MetricType::Gauge,
        None,
        vec![Measurement {
            labels: vec![],
            value: time_properties_ds.current_utc_offset.unwrap_or(0),
        }],
    )?;

    Ok(())
}

pub fn format_state(w: &mut impl std::fmt::Write, state: &ObservableState) -> std::fmt::Result {
    format_metric(
        w,
        "uptime",
        "The time that statime has been running",
        MetricType::Gauge,
        Some(Unit::Seconds),
        vec![Measurement {
            labels: vec![
                ("version", state.program.version.clone()),
                ("build_commit", state.program.build_commit.clone()),
                ("build_commit_date", state.program.build_commit_date.clone()),
            ],
            value: state.program.uptime_seconds,
        }],
    )?;

    format_default_ds(w, &state.instance.default_ds)?;
    format_time_properties_ds(w, &state.instance.time_properties_ds)?;

    w.write_str("# EOF\n")?;
    Ok(())
}

fn format_metric<T: std::fmt::Display>(
    w: &mut impl std::fmt::Write,
    name: &str,
    help: &str,
    metric_type: MetricType,
    unit: Option<Unit>,
    measurements: Vec<Measurement<T>>,
) -> std::fmt::Result {
    let name = if let Some(unit) = unit {
        format!("statime_{}_{}", name, unit.as_str())
    } else {
        format!("statime_{}", name)
    };

    // write help text
    writeln!(w, "# HELP {name} {help}.")?;

    // write type
    writeln!(w, "# TYPE {name} {}", metric_type.as_str())?;

    // write unit
    if let Some(unit) = unit {
        writeln!(w, "# UNIT {name} {}", unit.as_str())?;
    }

    // write all the measurements
    for measurement in measurements {
        w.write_str(&name)?;
        if !measurement.labels.is_empty() {
            w.write_str("{")?;

            for (offset, (label, value)) in measurement.labels.iter().enumerate() {
                let value = value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n");
                write!(w, "{label}=\"{value}\"")?;
                if offset < measurement.labels.len() - 1 {
                    w.write_str(",")?;
                }
            }
            w.write_str("}")?;
        }
        w.write_str(" ")?;
        write!(w, "{}", measurement.value)?;
        w.write_str("\n")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;

    use crate::metrics::exporter::Args;

    const BINARY: &str = "/usr/bin/statime-metrics-exporter";

    #[test]
    fn cli_config() {
        let config_str = "/foo/bar/statime.toml";
        let config = Path::new(config_str);
        let arguments = &[BINARY, "-c", config_str];

        let options = Args::try_parse_from(arguments).unwrap();
        assert_eq!(options.config.as_path(), config);
    }
}
