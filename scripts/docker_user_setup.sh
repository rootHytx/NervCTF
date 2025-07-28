if [ -z "$TARGET_IP" ]; then
    read -p "Enter your target machine IP address: " TARGET_IP
    if [ -z "$TARGET_IP" ]; then
        echo "TARGET_IP is not set. Please set it to the target machine's IP address."
        exit 1
    fi
fi

echo "This script will set up a docker user WITHOUT root privileges on the remote machine, at $TARGET_IP"
echo "For this script to work, the user account used for SSH must have sudo privileges (without the need for password)."
read -p "Do you want to continue? (y/n): " choice
if [[ "$choice" =~ ^[Yy]$ ]]; then
    read -p "What remote user do you have sudo access on? (Empty for 'root') " TARGET_USER
    if [ -z "$TARGET_USER" ]; then
        TARGET_USER="root"
    fi
    TARGET_HOST=${TARGET_USER}"@"${TARGET_IP}
else
    echo "Exiting script."
    exit 0
fi
ssh $TARGET_HOST "getent group docker &>/dev/null"
if [ $? -eq 0 ]; then
    echo "Group 'docker' exists on $TARGET_HOST."
else
    echo "Group 'docker' does NOT exist on $TARGET_HOST."
    echo "Since the group 'docker' is not present, it is assumed docker is not installed on the remote machine."
    read -p "Would you like to install docker on this remote target? (y/n) " key_choice

    if [[ "$key_choice" =~ ^[Yy]$ ]]; then
        echo "Installing Docker on $TARGET_HOST..."
        ssh $TARGET_HOST 'bash -s' < install_docker_on_remote.sh
        if [ $? -eq 0 ]; then
            echo "Docker installed successfully on $TARGET_HOST."
        else
            echo "Failed to install Docker on $TARGET_HOST. Please check the permissions or the SSH connection."
            exit 1
        fi
    else
        echo "Exiting without installing Docker."
        exit 0
    fi
fi

echo "Connecting to $TARGET_HOST to check for user 'docker'..."
ssh $TARGET_HOST "id -u docker &>/dev/null"

if [ $? -eq 0 ]; then
    echo "User 'docker' exists on the target machine."
else
    echo "User 'docker' does NOT exist on the target machine."
    echo "Creating user 'docker'..."
    ssh $TARGET_HOST "useradd -m docker -g docker && echo 'docker:docker' | chpasswd"
    if [ $? -eq 0 ]; then
        echo "User 'docker' created successfully."
    else
        echo "Failed to create user 'docker'. Please check the permissions or the SSH connection."
        exit 1
    fi
fi

echo
echo "Do you want to:"
echo "1) Create a new SSH key"
echo "2) Use an existing SSH public key"
read -p "Enter your choice (1 or 2): " key_choice

if [ "$key_choice" == "1" ]; then
    read -p "Enter a name for the new SSH key (e.g., id_rsa_docker): " key_name
    ssh-keygen -t rsa -b 4096 -f ~/.ssh/$key_name
    pubkey_path=~/.ssh/${key_name}.pub
elif [ "$key_choice" == "2" ]; then
    echo "Available public keys in ~/.ssh:"
    mapfile -t pubkeys < <(ls ~/.ssh/*.pub 2>/dev/null)
    if [ ${#pubkeys[@]} -eq 0 ]; then
        echo "No public keys found in ~/.ssh/. Please create one first."
        exit 1
    fi
    for i in "${!pubkeys[@]}"; do
        echo "$((i+1))) ${pubkeys[$i]##*/}"
    done
    read -p "Enter the number of the public key to use: " key_num
    if ! [[ "$key_num" =~ ^[0-9]+$ ]] || [ "$key_num" -lt 1 ] || [ "$key_num" -gt "${#pubkeys[@]}" ]; then
        echo "Invalid selection."
        exit 1
    fi
    pubkey_path="${pubkeys[$((key_num-1))]}"
else
    echo "Invalid choice. Exiting."
    exit 1
fi

if [ ! -f "$pubkey_path" ]; then
    echo "Public key file not found: $pubkey_path"
    exit 1
fi

echo "Copying public key to remote /home/docker/.ssh/authorized_keys..."
ssh $TARGET_HOST "mkdir -p /home/docker/.ssh && touch /home/docker/.ssh/authorized_keys && chown -R docker:docker /home/docker/.ssh"
cat "$pubkey_path" | ssh $TARGET_HOST "cat >> /home/docker/.ssh/authorized_keys && chown docker:docker /home/docker/.ssh/authorized_keys && chmod 600 /home/docker/.ssh/authorized_keys"

echo "Public key has been added to /home/docker/.ssh/authorized_keys on $TARGET_HOST."

echo "Placing systemd service for docker daemon exposure to the VPN network..."
touch tmp
echo "[Service]" > tmp
echo "ExecStart=" >> tmp
echo "ExecStart=/usr/bin/dockerd -H fd:// -H tcp://${TARGET_IP}:2375" >> tmp
ssh $TARGET_HOST "sudo mkdir -p /etc/systemd/system/docker.service.d"
scp -p tmp $TARGET_HOST:/etc/systemd/system/docker.service.d/override.conf
rm tmp
echo "Docker daemon configuration file has been placed on $TARGET_HOST."

echo "Restarting Docker daemon on $TARGET_HOST to apply changes..."
DOCKER_DAEMON_CONFIG="sudo systemctl daemon-reload && \
  sudo systemctl restart docker && \
  echo 'Docker daemon restarted successfully.' || echo 'Failed to restart Docker daemon.'"
ssh $TARGET_HOST "$DOCKER_DAEMON_CONFIG"

if [ $? -eq 0 ]; then
    echo "Docker daemon configuration updated successfully on $TARGET_HOST."
else
    echo "Failed to update Docker daemon configuration on $TARGET_HOST. Please check the permissions or the SSH connection."
    exit 1
fi
