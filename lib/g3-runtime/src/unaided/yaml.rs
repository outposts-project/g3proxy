/*
 * Copyright 2025 ByteDance and/or its affiliates.
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

use anyhow::{anyhow, Context};
use yaml_rust::Yaml;

use super::UnaidedRuntimeConfig;

impl UnaidedRuntimeConfig {
    pub fn parse_yaml(v: &Yaml) -> anyhow::Result<Self> {
        if let Yaml::Hash(map) = v {
            let mut config = UnaidedRuntimeConfig::default();
            #[cfg(all(unix, not(target_os = "openbsd")))]
            let mut set_mapped_sched_affinity = false;

            g3_yaml::foreach_kv(map, |k, v| match g3_yaml::key::normalize(k).as_str() {
                "thread_number" => {
                    let value = g3_yaml::value::as_usize(v)?;
                    config.set_thread_number(value);
                    Ok(())
                }
                "thread_stack_size" => {
                    let value = g3_yaml::humanize::as_usize(v)
                        .context(format!("invalid humanize usize value for key {k}"))?;
                    config.set_thread_stack_size(value);
                    Ok(())
                }
                #[cfg(all(unix, not(target_os = "openbsd")))]
                "sched_affinity" => {
                    if let Yaml::Hash(map) = v {
                        for (ik, iv) in map.iter() {
                            let id = g3_yaml::value::as_usize(ik)
                                .context(format!("the keys for {k} should be usize value"))?;
                            #[cfg(any(
                                target_os = "linux",
                                target_os = "android",
                                target_os = "freebsd",
                                target_os = "dragonfly",
                                target_os = "netbsd",
                            ))]
                            let cpu = g3_yaml::value::as_cpu_set(iv)
                                .context(format!("invalid cpu set value for {k}/{id}"))?;
                            #[cfg(target_os = "macos")]
                            let cpu = g3_yaml::value::as_cpu_tag(iv)
                                .context(format!("invalid cpu tag value for {k}/{id}"))?;

                            config.set_sched_affinity(id, cpu);
                        }
                        Ok(())
                    } else if let Ok(map_all) = g3_yaml::value::as_bool(v) {
                        set_mapped_sched_affinity = map_all;
                        Ok(())
                    } else {
                        Err(anyhow!("invalid map value for key {k}"))
                    }
                }
                "max_io_events_per_tick" => {
                    let capacity = g3_yaml::value::as_usize(v)?;
                    config.set_max_io_events_per_tick(capacity);
                    Ok(())
                }
                #[cfg(feature = "openssl-async-job")]
                "openssl_async_job_init_size" => {
                    let size = g3_yaml::value::as_usize(v)?;
                    config.set_openssl_async_job_init_size(size);
                    Ok(())
                }
                #[cfg(feature = "openssl-async-job")]
                "openssl_async_job_max_size" => {
                    let size = g3_yaml::value::as_usize(v)?;
                    config.set_openssl_async_job_init_size(size);
                    Ok(())
                }
                _ => Err(anyhow!("invalid key {k}")),
            })?;

            #[cfg(all(unix, not(target_os = "openbsd")))]
            if set_mapped_sched_affinity {
                config
                    .set_mapped_sched_affinity()
                    .context("failed to set all mapped sched affinity")?;
            }

            Ok(config)
        } else {
            Err(anyhow!(
                "yaml value type for 'unaided runtime config' should be 'map'"
            ))
        }
    }
}
