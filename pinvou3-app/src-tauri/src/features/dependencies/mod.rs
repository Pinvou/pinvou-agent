#[cfg(any(target_os = "linux", test))]
mod linux_packages;
mod platform;

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    platform::install_dependencies(packages)
}
