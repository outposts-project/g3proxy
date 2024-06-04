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

use std::sync::Arc;

use anyhow::anyhow;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::net::TcpStream;

use g3_daemon::stat::remote::{
    ArcTcpConnectionTaskRemoteStats, TcpConnectionTaskRemoteStatsWrapper,
};
use g3_http::connect::{HttpConnectRequest, HttpConnectResponse};
use g3_io_ext::{LimitedReader, LimitedStream, LimitedWriter};
use g3_openssl::SslConnector;
use g3_types::net::{Host, OpensslClientConfig};

use super::ProxyHttpEscaper;
use crate::log::escape::tls_handshake::{EscapeLogForTlsHandshake, TlsApplication};
use crate::module::tcp_connect::{
    TcpConnectError, TcpConnectRemoteWrapperStats, TcpConnectResult, TcpConnectTaskNotes,
};
use crate::serve::ServerTaskNotes;

impl ProxyHttpEscaper {
    pub(super) async fn http_connect_tcp_connect_to<'a>(
        &'a self,
        tcp_notes: &'a mut TcpConnectTaskNotes,
        task_notes: &'a ServerTaskNotes,
    ) -> Result<BufReader<LimitedStream<TcpStream>>, TcpConnectError> {
        let mut stream = self.tcp_new_connection(tcp_notes, task_notes).await?;

        let mut req =
            HttpConnectRequest::new(&tcp_notes.upstream, &self.config.append_http_headers);

        if self.config.pass_proxy_userid {
            if let Some(name) = task_notes.raw_user_name() {
                let line = crate::module::http_header::proxy_authorization_basic_pass(name);
                req.append_dyn_header(line);
            }
        }

        req.send(&mut stream)
            .await
            .map_err(TcpConnectError::NegotiationWriteFailed)?;

        let mut buf_stream = BufReader::new(stream);
        let _ =
            HttpConnectResponse::recv(&mut buf_stream, self.config.http_connect_rsp_hdr_max_size)
                .await?;

        // TODO detect and set outgoing_addr and target_addr for supported remote proxies

        Ok(buf_stream)
    }

    pub(super) async fn timed_http_connect_tcp_connect_to<'a>(
        &'a self,
        tcp_notes: &'a mut TcpConnectTaskNotes,
        task_notes: &'a ServerTaskNotes,
    ) -> Result<BufReader<LimitedStream<TcpStream>>, TcpConnectError> {
        tokio::time::timeout(
            self.config.peer_negotiation_timeout,
            self.http_connect_tcp_connect_to(tcp_notes, task_notes),
        )
        .await
        .map_err(|_| TcpConnectError::NegotiationPeerTimeout)?
    }

    pub(super) async fn http_connect_new_tcp_connection<'a>(
        &'a self,
        tcp_notes: &'a mut TcpConnectTaskNotes,
        task_notes: &'a ServerTaskNotes,
        task_stats: ArcTcpConnectionTaskRemoteStats,
    ) -> TcpConnectResult {
        let mut buf_stream = self
            .timed_http_connect_tcp_connect_to(tcp_notes, task_notes)
            .await?;

        // add in read buffered data
        let r_buffer_size = buf_stream.buffer().len() as u64;
        task_stats.add_read_bytes(r_buffer_size);
        let mut wrapper_stats = TcpConnectRemoteWrapperStats::new(&self.stats, task_stats);
        let user_stats = self.fetch_user_upstream_io_stats(task_notes);
        for s in &user_stats {
            s.io.tcp.add_in_bytes(r_buffer_size);
        }
        wrapper_stats.push_user_io_stats(user_stats);
        let wrapper_stats = Arc::new(wrapper_stats);

        // reset underlying io stats
        buf_stream.get_mut().reset_stats(wrapper_stats.clone());

        let (r, w) = tokio::io::split(buf_stream);
        Ok((Box::new(r), Box::new(w)))
    }

    pub(super) async fn http_connect_tls_connect_to<'a>(
        &'a self,
        tcp_notes: &'a mut TcpConnectTaskNotes,
        task_notes: &'a ServerTaskNotes,
        tls_config: &'a OpensslClientConfig,
        tls_name: &'a Host,
        tls_application: TlsApplication,
    ) -> Result<impl AsyncRead + AsyncWrite, TcpConnectError> {
        let buf_stream = self
            .timed_http_connect_tcp_connect_to(tcp_notes, task_notes)
            .await?;

        let ssl = tls_config
            .build_ssl(tls_name, tcp_notes.upstream.port())
            .map_err(TcpConnectError::InternalTlsClientError)?;
        let connector = SslConnector::new(ssl, buf_stream.into_inner())
            .map_err(|e| TcpConnectError::InternalTlsClientError(anyhow::Error::new(e)))?;

        match tokio::time::timeout(tls_config.handshake_timeout, connector.connect()).await {
            Ok(Ok(stream)) => Ok(stream),
            Ok(Err(e)) => {
                let e = anyhow::Error::new(e);
                EscapeLogForTlsHandshake {
                    tcp_notes,
                    task_id: &task_notes.id,
                    tls_name,
                    tls_peer: &tcp_notes.upstream,
                    tls_application,
                }
                .log(&self.escape_logger, &e);
                Err(TcpConnectError::UpstreamTlsHandshakeFailed(e))
            }
            Err(_) => {
                let e = anyhow!("upstream tls handshake timed out");
                EscapeLogForTlsHandshake {
                    tcp_notes,
                    task_id: &task_notes.id,
                    tls_name,
                    tls_peer: &tcp_notes.upstream,
                    tls_application,
                }
                .log(&self.escape_logger, &e);
                Err(TcpConnectError::UpstreamTlsHandshakeTimeout)
            }
        }
    }

    pub(super) async fn http_connect_new_tls_connection<'a>(
        &'a self,
        tcp_notes: &'a mut TcpConnectTaskNotes,
        task_notes: &'a ServerTaskNotes,
        task_stats: ArcTcpConnectionTaskRemoteStats,
        tls_config: &'a OpensslClientConfig,
        tls_name: &'a Host,
    ) -> TcpConnectResult {
        let tls_stream = self
            .http_connect_tls_connect_to(
                tcp_notes,
                task_notes,
                tls_config,
                tls_name,
                TlsApplication::TcpStream,
            )
            .await?;

        let (ups_r, ups_w) = tokio::io::split(tls_stream);

        // add task and user stats
        let mut wrapper_stats = TcpConnectionTaskRemoteStatsWrapper::new(task_stats);
        wrapper_stats.push_other_stats(self.fetch_user_upstream_io_stats(task_notes));
        let wrapper_stats = Arc::new(wrapper_stats);

        let ups_r = LimitedReader::new_unlimited(ups_r, wrapper_stats.clone() as _);
        let ups_w = LimitedWriter::new_unlimited(ups_w, wrapper_stats as _);

        Ok((Box::new(ups_r), Box::new(ups_w)))
    }
}
