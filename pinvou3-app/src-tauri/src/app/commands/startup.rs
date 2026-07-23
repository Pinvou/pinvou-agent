use super::prelude::*;
use crate::platform::startup as startup_domain;
use startup_domain::*;

sync_command_passthrough!(startup_domain, report_frontend_startup(entries: Vec<FrontendStartupEntry>));
