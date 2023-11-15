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

use std::collections::BTreeSet;

use anyhow::{anyhow, Context};
use bitflags::bitflags;
use yaml_rust::{yaml, Yaml};

use g3_types::acl::AclNetworkRuleBuilder;
use g3_types::metrics::MetricsName;
use g3_types::net::{RustlsServerConfigBuilder, UdpListenConfig};
use g3_yaml::YamlDocPosition;

use super::ServerConfig;
use crate::config::server::{AnyServerConfig, ServerConfigDiffAction};

const SERVER_CONFIG_TYPE: &str = "PlainQuicPort";

bitflags! {
    pub(crate) struct PlainQuicPortUpdateFlags: u64 {
        const LISTEN = 0b0001;
        const QUINN = 0b0010;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlainQuicPortConfig {
    name: MetricsName,
    position: Option<YamlDocPosition>,
    pub(crate) listen: UdpListenConfig,
    pub(crate) listen_in_worker: bool,
    pub(crate) tls_server: RustlsServerConfigBuilder,
    pub(crate) ingress_net_filter: Option<AclNetworkRuleBuilder>,
    pub(crate) server: MetricsName,
}

impl PlainQuicPortConfig {
    fn new(position: Option<YamlDocPosition>) -> Self {
        PlainQuicPortConfig {
            name: MetricsName::default(),
            position,
            listen: UdpListenConfig::default(),
            listen_in_worker: false,
            tls_server: RustlsServerConfigBuilder::empty(),
            ingress_net_filter: None,
            server: MetricsName::default(),
        }
    }

    pub(crate) fn parse(
        map: &yaml::Hash,
        position: Option<YamlDocPosition>,
    ) -> anyhow::Result<Self> {
        let mut server = PlainQuicPortConfig::new(position);

        g3_yaml::foreach_kv(map, |k, v| server.set(k, v))?;

        server.check()?;
        Ok(server)
    }

    fn set(&mut self, k: &str, v: &Yaml) -> anyhow::Result<()> {
        match g3_yaml::key::normalize(k).as_str() {
            super::CONFIG_KEY_SERVER_TYPE => Ok(()),
            super::CONFIG_KEY_SERVER_NAME => {
                self.name = g3_yaml::value::as_metrics_name(v)?;
                Ok(())
            }
            "listen" => {
                self.listen = g3_yaml::value::as_udp_listen_config(v)
                    .context(format!("invalid udp listen config value for key {k}"))?;
                Ok(())
            }
            "listen_in_worker" => {
                self.listen_in_worker = g3_yaml::value::as_bool(v)?;
                Ok(())
            }
            "quic_server" => {
                let lookup_dir = g3_daemon::config::get_lookup_dir(self.position.as_ref())?;
                self.tls_server =
                    g3_yaml::value::as_rustls_server_config_builder(v, Some(lookup_dir))?;
                Ok(())
            }
            "ingress_network_filter" | "ingress_net_filter" => {
                let filter = g3_yaml::value::acl::as_ingress_network_rule_builder(v).context(
                    format!("invalid ingress network acl rule value for key {k}"),
                )?;
                self.ingress_net_filter = Some(filter);
                Ok(())
            }
            "server" => {
                self.server = g3_yaml::value::as_metrics_name(v)?;
                Ok(())
            }
            _ => Err(anyhow!("invalid key {k}")),
        }
    }

    fn check(&mut self) -> anyhow::Result<()> {
        if self.name.is_empty() {
            return Err(anyhow!("name is not set"));
        }
        if self.server.is_empty() {
            return Err(anyhow!("server is not set"));
        }
        // make sure listen is always set
        self.listen.check().context("invalid listen config")?;
        self.tls_server.check().context("invalid quic tls config")?;

        Ok(())
    }
}

impl ServerConfig for PlainQuicPortConfig {
    fn name(&self) -> &MetricsName {
        &self.name
    }

    fn position(&self) -> Option<YamlDocPosition> {
        self.position.clone()
    }

    fn server_type(&self) -> &'static str {
        SERVER_CONFIG_TYPE
    }

    fn escaper(&self) -> &MetricsName {
        Default::default()
    }

    fn user_group(&self) -> &MetricsName {
        Default::default()
    }

    fn auditor(&self) -> &MetricsName {
        Default::default()
    }

    fn diff_action(&self, new: &AnyServerConfig) -> ServerConfigDiffAction {
        let new = match new {
            AnyServerConfig::PlainQuicPort(config) => config,
            _ => return ServerConfigDiffAction::SpawnNew,
        };

        if self.eq(new) {
            return ServerConfigDiffAction::NoAction;
        }

        let mut flags = PlainQuicPortUpdateFlags::empty();
        if self.listen != new.listen {
            flags.set(PlainQuicPortUpdateFlags::LISTEN, true);
        }
        if self.tls_server != new.tls_server {
            flags.set(PlainQuicPortUpdateFlags::QUINN, true);
        }

        ServerConfigDiffAction::UpdateInPlace(flags.bits())
    }

    fn dependent_server(&self) -> Option<BTreeSet<MetricsName>> {
        let mut set = BTreeSet::new();
        set.insert(self.server.clone());
        Some(set)
    }
}
