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

use std::env;

fn main() {
    g3_build_env::check_basic();
    g3_build_env::check_openssl();
    g3_build_env::check_rustls_provider();

    if env::var("CARGO_FEATURE_QUIC").is_ok() {
        println!("cargo:rustc-env=G3_QUIC_FEATURE=quinn");
    }
}
