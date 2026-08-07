use super::prelude::*;
use crate::features::files::file_ingest as dependencies_domain;
use dependencies_domain::*;

sync_command_passthrough!(dependencies_domain, check_dependencies() -> Vec<DependencyCheckItem>);
async_command_passthrough!(dependencies_domain, install_dependencies(app: AppHandle, packages: Vec<String>) -> Result<(), String>);
