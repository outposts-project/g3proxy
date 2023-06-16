/*
 * Copyright 2023 ByteDance and/or its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use bytes::Bytes;
use clap::{value_parser, Arg, ArgAction, ArgMatches, Command};
use h3::client::SendRequest;
use h3_quinn::OpenStreams;
use http::{HeaderValue, Method, StatusCode};
use url::Url;

use g3_types::collection::{SelectiveVec, WeightedValue};
use g3_types::net::{HttpAuth, RustlsClientConfigBuilder, UpstreamAddr};

use super::{H3PreRequest, HttpRuntimeStats, ProcArgs};

const HTTP_ARG_CONNECTION_POOL: &str = "connection-pool";
const HTTP_ARG_URI: &str = "uri";
const HTTP_ARG_METHOD: &str = "method";
const HTTP_ARG_LOCAL_ADDRESS: &str = "local-address";
const HTTP_ARG_NO_MULTIPLEX: &str = "no-multiplex";
const HTTP_ARG_OK_STATUS: &str = "ok-status";
const HTTP_ARG_TIMEOUT: &str = "timeout";
const HTTP_ARG_CONNECT_TIMEOUT: &str = "connect-timeout";

pub(super) struct BenchH3Args {
    pub(super) pool_size: Option<usize>,
    pub(super) method: Method,
    target_url: Url,
    bind: Option<IpAddr>,
    pub(super) no_multiplex: bool,
    pub(super) ok_status: Option<StatusCode>,
    pub(super) timeout: Duration,
    pub(super) connect_timeout: Duration,

    host: UpstreamAddr,
    auth: HttpAuth,
    peer_addrs: SelectiveVec<WeightedValue<SocketAddr>>,
}

impl BenchH3Args {
    fn new(url: Url) -> anyhow::Result<Self> {
        let upstream = UpstreamAddr::try_from(&url)?;
        let auth = HttpAuth::try_from(&url)
            .map_err(|e| anyhow!("failed to detect upstream auth method: {e}"))?;

        Ok(BenchH3Args {
            pool_size: None,
            method: Method::GET,
            target_url: url,
            bind: None,
            no_multiplex: false,
            ok_status: None,
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(15),
            host: upstream,
            auth,
            peer_addrs: SelectiveVec::empty(),
        })
    }

    pub(super) async fn resolve_target_address(
        &mut self,
        proc_args: &ProcArgs,
    ) -> anyhow::Result<()> {
        self.peer_addrs = proc_args.resolve(&self.host).await?;
        Ok(())
    }

    async fn new_quic_connection(
        &self,
        proc_args: &ProcArgs,
    ) -> anyhow::Result<h3_quinn::Connection> {
        let peer = *proc_args.select_peer(&self.peer_addrs);

        let endpoint = h3_quinn::quinn::Endpoint::client(peer)
            .map_err(|e| anyhow!("failed to create quic endpoint: {e}"))?;

        let tls_config_builder = RustlsClientConfigBuilder::default();
        let tls_config = tls_config_builder.build()?;
        let client_config = h3_quinn::quinn::ClientConfig::new(tls_config.driver);

        let conn = endpoint
            .connect_with(client_config, peer, "")
            .map_err(|e| anyhow!("failed to create quic client: {e}"))?
            .await
            .map_err(|e| anyhow!("failed to connect: {e}"))?;
        Ok(h3_quinn::Connection::new(conn))
    }

    pub(super) async fn new_h3_connection(
        &self,
        _stats: &Arc<HttpRuntimeStats>,
        proc_args: &ProcArgs,
    ) -> anyhow::Result<SendRequest<OpenStreams, Bytes>> {
        let quic_conn = self.new_quic_connection(proc_args).await?;

        let (mut driver, send_request) = h3::client::new(quic_conn)
            .await
            .map_err(|e| anyhow!("failed to create h3 connection: {e}"))?;
        tokio::spawn(async move {
            let _ = driver.wait_idle().await;
        });

        Ok(send_request)
    }

    pub(super) fn build_pre_request_header(&self) -> anyhow::Result<H3PreRequest> {
        let path_and_query = if let Some(q) = self.target_url.query() {
            format!("{}?{q}", self.target_url.path())
        } else {
            self.target_url.path().to_string()
        };
        let uri = http::Uri::builder()
            .scheme(self.target_url.scheme())
            .authority(self.host.to_string())
            .path_and_query(path_and_query)
            .build()
            .map_err(|e| anyhow!("failed to build request: {e:?}"))?;

        let host_str = self.host.to_string();
        let host =
            HeaderValue::from_str(&host_str).map_err(|e| anyhow!("invalid host value: {e:?}"))?;

        let auth = match &self.auth {
            HttpAuth::None => None,
            HttpAuth::Basic(basic) => {
                let value = format!("Basic {}", basic.encoded_value());
                let value = HeaderValue::from_str(&value)
                    .map_err(|e| anyhow!("invalid auth value: {e:?}"))?;
                Some(value)
            }
        };

        Ok(H3PreRequest {
            method: self.method.clone(),
            uri,
            host,
            auth,
        })
    }
}

pub(super) fn add_h3_args(app: Command) -> Command {
    app.arg(Arg::new(HTTP_ARG_URI).required(true).num_args(1))
        .arg(
            Arg::new(HTTP_ARG_CONNECTION_POOL)
                .help(
                    "Set the number of pooled underlying h3 connections.\n\
                        If not set, each concurrency will use it's own h3 connection",
                )
                .value_name("POOL SIZE")
                .long(HTTP_ARG_CONNECTION_POOL)
                .short('C')
                .num_args(1)
                .value_parser(value_parser!(usize))
                .conflicts_with(HTTP_ARG_NO_MULTIPLEX),
        )
        .arg(
            Arg::new(HTTP_ARG_METHOD)
                .value_name("METHOD")
                .short('m')
                .long(HTTP_ARG_METHOD)
                .num_args(1)
                .value_parser(["GET", "HEAD"])
                .default_value("GET"),
        )
        .arg(
            Arg::new(HTTP_ARG_LOCAL_ADDRESS)
                .value_name("LOCAL IP ADDRESS")
                .short('B')
                .long(HTTP_ARG_LOCAL_ADDRESS)
                .num_args(1)
                .value_parser(value_parser!(IpAddr)),
        )
        .arg(
            Arg::new(HTTP_ARG_NO_MULTIPLEX)
                .help("Disable h3 connection multiplexing")
                .action(ArgAction::SetTrue)
                .long(HTTP_ARG_NO_MULTIPLEX)
                .conflicts_with(HTTP_ARG_CONNECTION_POOL),
        )
        .arg(
            Arg::new(HTTP_ARG_OK_STATUS)
                .help("Only treat this status code as success")
                .value_name("STATUS CODE")
                .long(HTTP_ARG_OK_STATUS)
                .num_args(1)
                .value_parser(value_parser!(StatusCode)),
        )
        .arg(
            Arg::new(HTTP_ARG_TIMEOUT)
                .help("Http response timeout")
                .value_name("TIMEOUT DURATION")
                .default_value("30s")
                .long(HTTP_ARG_TIMEOUT)
                .num_args(1),
        )
        .arg(
            Arg::new(HTTP_ARG_CONNECT_TIMEOUT)
                .help("Timeout for connection to next peer")
                .value_name("TIMEOUT DURATION")
                .default_value("15s")
                .long(HTTP_ARG_CONNECT_TIMEOUT)
                .num_args(1),
        )
}

pub(super) fn parse_h3_args(args: &ArgMatches) -> anyhow::Result<BenchH3Args> {
    let url = if let Some(v) = args.get_one::<String>(HTTP_ARG_URI) {
        Url::parse(v).context(format!("invalid {HTTP_ARG_URI} value"))?
    } else {
        return Err(anyhow!("no target url set"));
    };

    let mut h3_args = BenchH3Args::new(url)?;

    if let Some(c) = args.get_one::<usize>(HTTP_ARG_CONNECTION_POOL) {
        if *c > 0 {
            h3_args.pool_size = Some(*c);
        }
    }

    if let Some(v) = args.get_one::<String>(HTTP_ARG_METHOD) {
        let method = Method::from_str(v).context(format!("invalid {HTTP_ARG_METHOD} value"))?;
        h3_args.method = method;
    }

    if let Some(ip) = args.get_one::<IpAddr>(HTTP_ARG_LOCAL_ADDRESS) {
        h3_args.bind = Some(*ip);
    }

    if args.get_flag(HTTP_ARG_NO_MULTIPLEX) {
        h3_args.no_multiplex = true;
    }

    if let Some(code) = args.get_one::<StatusCode>(HTTP_ARG_OK_STATUS) {
        h3_args.ok_status = Some(*code);
    }

    if let Some(timeout) = g3_clap::humanize::get_duration(args, HTTP_ARG_TIMEOUT)? {
        h3_args.timeout = timeout;
    }

    if let Some(timeout) = g3_clap::humanize::get_duration(args, HTTP_ARG_CONNECT_TIMEOUT)? {
        h3_args.connect_timeout = timeout;
    }

    match h3_args.target_url.scheme() {
        "http" | "https" => {}
        _ => return Err(anyhow!("unsupported target url {}", h3_args.target_url)),
    }

    Ok(h3_args)
}
