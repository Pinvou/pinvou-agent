mod platform;

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    platform::install_dependencies(packages)
}
