use anyhow::{anyhow, Result};

/// Bring up a Vagrant VM for a team instance.
///
/// Returns `(host_port, vm_id)`.
pub async fn up(
    _vagrantfile_dir: &str,
    _vm_name: &str,
    _internal_port: u32,
) -> Result<(u16, String)> {
    Err(anyhow!(
        "Vagrant backend is not yet configured on this server. \
         Install Vagrant and libvirt/VirtualBox before using Vagrant challenges."
    ))
}

