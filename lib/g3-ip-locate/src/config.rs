/*
 * Copyright 2024 ByteDance and/or its affiliates.
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

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::anyhow;
use tokio::net::UdpSocket;

use g3_types::net::SocketBufferConfig;

use super::{IpLocationQueryRuntime, IpLocationServiceHandle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpLocationServiceConfig {
    pub(crate) cache_request_batch_count: usize,
    pub(crate) cache_request_timeout: Duration,
    pub(crate) query_peer_addr: SocketAddr,
    pub(crate) query_socket_buffer: SocketBufferConfig,
    pub(crate) query_wait_timeout: Duration,
    pub(crate) default_expire_ttl: u32,
    pub(crate) maximum_expire_ttl: u32,
}

impl Default for IpLocationServiceConfig {
    fn default() -> Self {
        IpLocationServiceConfig {
            cache_request_batch_count: 10,
            cache_request_timeout: Duration::from_millis(800),
            query_peer_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2888),
            query_socket_buffer: SocketBufferConfig::default(),
            query_wait_timeout: Duration::from_millis(400),
            default_expire_ttl: 10,
            maximum_expire_ttl: 300,
        }
    }
}

impl IpLocationServiceConfig {
    pub fn set_cache_request_batch_count(&mut self, count: usize) {
        self.cache_request_batch_count = count;
    }

    pub fn set_cache_request_timeout(&mut self, time: Duration) {
        self.cache_request_timeout = time;
    }

    pub fn set_query_peer_addr(&mut self, addr: SocketAddr) {
        self.query_peer_addr = addr;
    }

    pub fn set_query_socket_buffer(&mut self, config: SocketBufferConfig) {
        self.query_socket_buffer = config;
    }

    pub fn set_query_wait_timeout(&mut self, time: Duration) {
        self.query_wait_timeout = time;
    }

    pub fn set_default_expire_ttl(&mut self, ttl: u32) {
        self.default_expire_ttl = ttl;
    }

    pub fn set_maximum_expire_ttl(&mut self, ttl: u32) {
        self.maximum_expire_ttl = ttl;
    }

    pub fn spawn_cert_agent(&self) -> anyhow::Result<IpLocationServiceHandle> {
        use anyhow::Context;

        let (socket, _addr) = g3_socket::udp::new_std_bind_connect(
            None,
            self.query_socket_buffer,
            Default::default(),
        )
        .context("failed to setup udp socket")?;
        socket.connect(self.query_peer_addr).map_err(|e| {
            anyhow!(
                "failed to connect to peer address {}: {e:?}",
                self.query_peer_addr
            )
        })?;
        let socket = UdpSocket::from_std(socket).context("failed to setup udp socket")?;

        let (cache_runtime, cache_handle, query_handle) = super::spawn_ip_location_cache(self);
        let query_runtime = IpLocationQueryRuntime::new(self, socket, query_handle);

        tokio::spawn(query_runtime);
        tokio::spawn(cache_runtime);

        Ok(IpLocationServiceHandle::new(
            cache_handle,
            self.cache_request_timeout,
        ))
    }
}
