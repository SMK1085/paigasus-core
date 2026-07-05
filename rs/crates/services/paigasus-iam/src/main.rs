// SPDX-License-Identifier: Apache-2.0

//! paigasus-iam service entrypoint. Full composition root lands in SMA-441 Task 11.

mod adapters;
mod application;
mod config;

fn main() {
    // Placeholder: real wiring (logging, DB, servers, shutdown) added in Task 11.
    let _ = config::IamConfig::figment();
}
