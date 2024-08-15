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

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use slog::{slog_o, Logger, OwnedKV, SendSyncRefUnwindSafeKV};
use yaml_rust::Yaml;

use g3_fluentd::FluentdClientConfig;
#[cfg(target_os = "linux")]
use g3_journal::JournalConfig;
use g3_syslog::SyslogBuilder;
use g3_types::log::AsyncLogConfig;

use super::{LoggerStats, ReportLogIoError};

const DEFAULT_CHANNEL_SIZE: usize = 4096;
const IO_ERROR_SAMPLING_OFFSET_MAX: usize = 16;
const IO_ERROR_SAMPLING_OFFSET_DEFAULT: usize = 10;

#[derive(Clone)]
pub enum LogConfigDriver {
    Discard,
    #[cfg(target_os = "linux")]
    Journal(JournalConfig),
    Syslog(SyslogBuilder),
    Fluentd(Arc<FluentdClientConfig>),
    Stdout,
}

#[derive(Clone)]
pub struct LogConfig {
    pub(crate) driver: LogConfigDriver,
    pub(crate) async_channel_size: usize,
    pub(crate) async_thread_number: usize,
    pub(crate) io_err_sampling_mask: usize,
    pub(crate) program_name: &'static str,
}

impl LogConfig {
    fn with_driver(driver: LogConfigDriver, program_name: &'static str) -> Self {
        LogConfig {
            driver,
            async_channel_size: DEFAULT_CHANNEL_SIZE,
            async_thread_number: 1,
            io_err_sampling_mask: (1 << IO_ERROR_SAMPLING_OFFSET_DEFAULT) - 1,
            program_name,
        }
    }

    pub fn with_driver_name(driver: &str, program_name: &'static str) -> anyhow::Result<Self> {
        match driver {
            "discard" => Ok(LogConfig::new_discard(program_name)),
            #[cfg(target_os = "linux")]
            "journal" => Ok(LogConfig::new_journal(program_name)),
            "syslog" => Ok(LogConfig::new_syslog(program_name)),
            "fluentd" => Ok(LogConfig::new_fluentd(program_name)),
            "stdout" => Ok(LogConfig::new_stdout(program_name)),
            _ => Err(anyhow!("invalid default log config")),
        }
    }

    pub fn new_discard(program_name: &'static str) -> Self {
        Self::with_driver(LogConfigDriver::Discard, program_name)
    }

    #[cfg(target_os = "linux")]
    pub fn new_journal(program_name: &'static str) -> Self {
        Self::with_driver(
            LogConfigDriver::Journal(JournalConfig::with_ident(program_name)),
            program_name,
        )
    }

    pub fn new_syslog(program_name: &'static str) -> Self {
        Self::with_driver(
            LogConfigDriver::Syslog(SyslogBuilder::with_ident(program_name)),
            program_name,
        )
    }

    pub fn new_fluentd(program_name: &'static str) -> Self {
        Self::with_driver(
            LogConfigDriver::Fluentd(Arc::new(FluentdClientConfig::default())),
            program_name,
        )
    }

    pub fn new_stdout(program_name: &'static str) -> Self {
        Self::with_driver(LogConfigDriver::Stdout, program_name)
    }

    pub fn parse_yaml(
        v: &Yaml,
        conf_dir: &Path,
        program_name: &'static str,
    ) -> anyhow::Result<LogConfig> {
        match v {
            Yaml::String(s) => match s.as_str() {
                "discard" => Ok(LogConfig::new_discard(program_name)),
                #[cfg(target_os = "linux")]
                "journal" => Ok(LogConfig::new_journal(program_name)),
                "syslog" => Ok(LogConfig::new_syslog(program_name)),
                "fluentd" => Ok(LogConfig::new_fluentd(program_name)),
                "stdout" => Ok(LogConfig::new_stdout(program_name)),
                _ => Err(anyhow!("invalid log config")),
            },
            Yaml::Hash(map) => {
                let mut config = LogConfig::new_discard(program_name);
                g3_yaml::foreach_kv(map, |k, v| match g3_yaml::key::normalize(k).as_str() {
                    #[cfg(target_os = "linux")]
                    "journal" => {
                        config.driver =
                            LogConfigDriver::Journal(JournalConfig::with_ident(program_name));
                        Ok(())
                    }
                    "syslog" => {
                        let builder = g3_yaml::value::as_syslog_builder(v, program_name)
                            .context("invalid syslog config")?;
                        config.driver = LogConfigDriver::Syslog(builder);
                        Ok(())
                    }
                    "fluentd" => {
                        let client = g3_yaml::value::as_fluentd_client_config(v, Some(conf_dir))
                            .context("invalid fluentd config")?;
                        config.driver = LogConfigDriver::Fluentd(Arc::new(client));
                        Ok(())
                    }
                    "async_channel_size" | "channel_size" => {
                        let channel_size = g3_yaml::value::as_usize(v)
                            .context(format!("invalid usize value for key {k}"))?;
                        config.async_channel_size = channel_size;
                        Ok(())
                    }
                    "async_thread_number" | "thread_number" => {
                        let thread_number = g3_yaml::value::as_usize(v)
                            .context(format!("invalid usize value for key {k}"))?;
                        config.async_thread_number = thread_number;
                        Ok(())
                    }
                    "io_error_sampling_offset" => {
                        let offset = g3_yaml::value::as_usize(v)
                            .context(format!("invalid value for key {k}"))?;
                        if offset > IO_ERROR_SAMPLING_OFFSET_MAX {
                            Err(anyhow!(
                                "value for {k} should be less than {IO_ERROR_SAMPLING_OFFSET_MAX}"
                            ))
                        } else {
                            config.io_err_sampling_mask = (1 << offset) - 1;
                            Ok(())
                        }
                    }
                    _ => Err(anyhow!("invalid key {k}")),
                })?;
                Ok(config)
            }
            _ => Err(anyhow!("invalid value type")),
        }
    }

    pub fn build_shared_logger(
        self,
        logger_name: String,
        daemon_group: &'static str,
        log_type: &'static str,
    ) -> Logger {
        let common_values = slog_o!(
            "daemon_name" => daemon_group,
            "log_type" => log_type,
            "pid" => std::process::id(),
        );
        self.build_logger(logger_name, log_type, common_values)
    }

    pub fn build_logger<T>(
        self,
        logger_name: String,
        log_type: &'static str,
        common_values: OwnedKV<T>,
    ) -> Logger
    where
        T: SendSyncRefUnwindSafeKV + 'static,
    {
        let async_conf = AsyncLogConfig {
            channel_capacity: self.async_channel_size,
            thread_number: self.async_thread_number,
            thread_name: logger_name.clone(),
        };

        match self.driver {
            LogConfigDriver::Discard => {
                let drain = slog::Discard {};
                Logger::root(drain, common_values)
            }
            #[cfg(target_os = "linux")]
            LogConfigDriver::Journal(journal_conf) => {
                let drain = g3_journal::new_async_logger(&async_conf, journal_conf);
                let logger_stats = LoggerStats::new(&logger_name, drain.get_stats());
                super::registry::add(logger_name.clone(), Arc::new(logger_stats));
                let drain = ReportLogIoError::new(drain, &logger_name, self.io_err_sampling_mask);
                Logger::root(drain, common_values)
            }
            LogConfigDriver::Syslog(builder) => {
                let drain = builder.start_async(&async_conf);
                let logger_stats = LoggerStats::new(&logger_name, drain.get_stats());
                super::registry::add(logger_name.clone(), Arc::new(logger_stats));
                let drain = ReportLogIoError::new(drain, &logger_name, self.io_err_sampling_mask);
                Logger::root(drain, common_values)
            }
            LogConfigDriver::Fluentd(fluentd_conf) => {
                let drain = g3_fluentd::new_async_logger(
                    &async_conf,
                    &fluentd_conf,
                    format!("{}.{log_type}", self.program_name),
                );
                let logger_stats = LoggerStats::new(&logger_name, drain.get_stats());
                super::registry::add(logger_name.clone(), Arc::new(logger_stats));
                let drain = ReportLogIoError::new(drain, &logger_name, self.io_err_sampling_mask);
                Logger::root(drain, common_values)
            }
            LogConfigDriver::Stdout => {
                let drain = g3_stdlog::new_async_logger(&async_conf, false, true);
                let logger_stats = LoggerStats::new(&logger_name, drain.get_stats());
                super::registry::add(logger_name.clone(), Arc::new(logger_stats));
                let drain = slog::IgnoreResult::new(drain);
                Logger::root(drain, common_values)
            }
        }
    }
}
