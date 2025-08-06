# NervCTF Scripts

This directory contains utility scripts for setting up and managing CTFd and Docker environments for the NervCTF platform. Below you'll find descriptions and usage instructions for each script.

---

## 1. `docker_user_setup.sh`

**Purpose:**
Automates the setup of a non-root `docker` user on a remote machine, configures SSH access, and exposes the Docker daemon over the VPN network.

**Features:**
- Prompts for the target machine IP and remote user (with sudo privileges).
- Checks for the existence of the `docker` group and installs Docker if missing.
- Creates a `docker` user and sets up its shell and environment.
- Allows you to generate a new SSH key or use an existing one for secure access.
- Copies the selected public key to `/home/docker/.ssh/authorized_keys` on the remote machine.
- Configures the Docker daemon to listen on the VPN IP via a systemd override.
- Restarts the Docker daemon to apply changes.

**Usage:**
```bash
./docker_user_setup.sh
```
Follow the interactive prompts to complete the setup.

---

## 2. `install_webhook_plugin.sh`

**Purpose:**
Searches for CTFd installations on the system, allows the user to select the correct instance, and installs the [ctfd-solve-webhook-plugin](https://github.com/iosifache/ctfd-solve-webhook-plugin) into the appropriate plugins directory.

**Features:**
- Searches the filesystem for directories named `CTFd` that contain a nested `CTFd` subdirectory.
- Lists all matching CTFd paths and lets the user select one.
- Clones the webhook plugin into the selected CTFd instance's plugins directory.
- Optionally configures the plugin with a provided webhook URL.

**Usage:**
```bash
./install_webhook_plugin.sh [WEBHOOK_URL]
```
- If `WEBHOOK_URL` is provided as an argument, it will be configured for the plugin.
- Follow the interactive prompts to select the correct CTFd instance.

---

## 3. `install_docker_on_remote.sh`

**Purpose:**
Installs Docker and related components on an Ubuntu-based remote machine.

**Features:**
- Removes any conflicting Docker or container packages.
- Adds Docker's official GPG key and repository.
- Installs Docker Engine, CLI, Compose plugin, and related tools.

**Usage:**
This script is intended to be run remotely via SSH by `docker_user_setup.sh`.
You can also run it directly on an Ubuntu machine:
```bash
sudo bash install_docker_on_remote.sh
```

---

## Notes

- All scripts are designed for Ubuntu-based systems.
- Ensure you have the necessary permissions and SSH access to the target machines.
- For best results, run these scripts from a user account with sudo privileges and SSH key-based authentication.

---

## Troubleshooting

- If you encounter permission errors, verify your SSH user has passwordless sudo access.
- For Docker daemon exposure, ensure your VPN IP is correctly configured.
- If the plugin installation fails, check that the selected CTFd instance is valid and writable.

---

## License

These scripts are provided as-is for educational and operational use in the NervCTF environment.
