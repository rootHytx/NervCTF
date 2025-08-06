show_spinner() {
    local pid=$1
    local delay=0.1
    local spinstr='|/-\'
    while [ "$(ps a | awk '{print $1}' | grep $pid)" ]; do
        local temp=${spinstr#?}
        printf " [%c] %s  " "$spinstr" "$message"
        spinstr=$temp${spinstr%"$temp"}
        sleep $delay
        printf "\r"
    done
    printf "    \r"
}
search_remote_keys(){
    REMOTE_AUTH_KEYS="/home/docker/.ssh/authorized_keys"

    # List your local public keys
    mapfile -t pubkeys < <(ls ~/.ssh/*.pub 2>/dev/null)
    if [ ${#pubkeys[@]} -eq 0 ]; then
        echo "No public keys found in ~/.ssh/"
        return 1
    fi

    # Check if any local key is present on the remote
    key_found=false
    for keyfile in "${pubkeys[@]}"; do
        key_content=$(cat "$keyfile")
        # Use grep on the remote authorized_keys
        ssh "$TARGET_HOST" "grep -Fxq '$key_content' $REMOTE_AUTH_KEYS" && key_found=true && break
    done

    if $key_found; then
        echo "One of your public keys is already present on the remote machine. Skipping key addition."
        return 0
    else
        echo "No matching public key found on the remote machine."
        return 1
    fi

}
search_remote_ctfd() {
    # Parse results: only keep directories that have a nested CTFd
    mapfile -t ctfd_paths < <(
        ssh "$TARGET_HOST" '
            tmpfile=$(mktemp)
            find / -type d -name "CTFd" 2>/dev/null > "$tmpfile"
            while IFS= read -r parent_dir; do
                if [ -d "$parent_dir/CTFd" ]; then
                    echo "$parent_dir" > ctfd_dir
                fi
            done < "$tmpfile"
            rm "$tmpfile"
        '
    ) &
    find_pid=$!
    show_spinner $find_pid "Searching for CTFd folders..."
    wait $find_pid
    scp $TARGET_HOST:ctfd_dir . 2>/dev/null
    mapfile -t ctfd_paths < ctfd_dir
    rm ctfd_dir
    ssh $TARGET_HOST "rm -f ctfd_dir"
    if [ ${#ctfd_paths[@]} -eq 0 ]; then
        echo "No matching CTFd directories found on remote."
        return 1
    fi

    echo "Select the CTFd directory you want to use (on remote):"
    for i in "${!ctfd_paths[@]}"; do
        echo "$((i+1)). ${ctfd_paths[$i]}"
    done

    read -p "Enter the number of your choice: " choice

    if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "${#ctfd_paths[@]}" ]; then
        CTFD_DIR="${ctfd_paths[$((choice-1))]}"
        export CTFD_DIR
        return 0
    else
        echo "Invalid selection."
        return 1
    fi
}

echo "This script is intended to use BEFORE using NervCTF"
echo "It will go through a multi-stage process installing the necessary components to run NervCTF on a remote machine."

read -p "Enter your target machine IP address: " TARGET_IP
if [ -z "$TARGET_IP" ]; then
    echo "TARGET_IP is not set. Please set it to the target machine's IP address."
    exit 1
fi

read -p "What remote user do you have sudo access on? (Empty for 'root') " TARGET_USER
if [ -z "$TARGET_USER" ]; then
    TARGET_USER="root"
fi
TARGET_HOST="${TARGET_USER}@${TARGET_IP}"

# Generate a temporary inventory file
INVENTORY=$(mktemp)
echo "[ctfd]" > "$INVENTORY"
echo "$TARGET_IP ansible_user=$TARGET_USER" >> "$INVENTORY"

PLAYBOOK="./scripts/nervctf_playbook.yml"

# 1) Docker group and install
ssh $TARGET_HOST "getent group docker &>/dev/null"
if [ $? -eq 0 ]; then
    echo "Docker group exists on $TARGET_HOST."
else
    echo "Docker group does NOT exist. Running Ansible docker_group, docker_dependencies, docker_gpg, docker_repo, docker_install tasks..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "docker_group,docker_dependencies,docker_gpg,docker_repo,docker_install"
fi

# 2) Docker user
ssh $TARGET_HOST "id -u docker &>/dev/null"
if [ $? -eq 0 ]; then
    echo "User 'docker' exists on $TARGET_HOST."
else
    echo "User 'docker' does NOT exist. Running Ansible docker_user task..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "docker_user"
fi

# 3) SSH key for docker user
search_remote_keys
if [ $? -eq 0 ]; then
    echo "SSH key already exists for docker user."
else
    ssh $TARGET_HOST "test -f /home/docker/.ssh/authorized_keys"
    if [ $? -eq 0 ]; then
        echo "SSH authorized_keys exists for docker user."
    else
        echo "No SSH authorized_keys for docker user. Running Ansible docker_ssh task..."
        ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "docker_ssh"
    fi
fi


# 4) .bashrc TERM setting
ssh $TARGET_HOST "grep -q 'TERM=xterm' /home/docker/.bashrc"
if [ $? -eq 0 ]; then
    echo "TERM=xterm already set in .bashrc."
else
    echo "Setting TERM=xterm in .bashrc. Running Ansible docker_bashrc task..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "docker_bashrc"
fi

# 5) Docker systemd override
ssh $TARGET_HOST "test -f /etc/systemd/system/docker.service.d/override.conf"
if [ $? -eq 0 ]; then
    echo "Docker systemd override already exists."
else
    echo "Placing Docker systemd override. Running Ansible docker_systemd_dir,docker_systemd_override tasks..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "docker_systemd_dir,docker_systemd_override"
    echo "Reloading and restarting Docker daemon. Running Ansible systemd_reload,docker_restart tasks..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "systemd_reload,docker_restart"
fi

# 6) CTFd install
echo "Searching for existing CTFd directories on the remote machine..."
search_remote_ctfd
if [ $? -ne 0 ]; then
    echo "No CTFd directory found. Installing CTFd on remote host..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "ctfd_clone"
    echo "CTFd installation complete."
fi

# 7) Plugins directory
ssh $TARGET_HOST "test -d $CTFD_DIR/CTFd/plugins"
if [ $? -eq 0 ]; then
    echo "CTFd plugins directory exists."
else
    echo "Creating plugins directory. Running Ansible ctfd_plugins_dir task..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "ctfd_plugins_dir"
fi

# 8) Webhook plugin
ssh $TARGET_HOST "test -d $CTFD_DIR/CTFd/plugins/solve_webhook"
if [ $? -eq 0 ]; then
    echo "Webhook plugin already installed."
else
    echo "Installing webhook plugin. Running Ansible webhook_plugin task..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "webhook_plugin"
fi

# 9) Webhook plugin config
ssh $TARGET_HOST "test -f $CTFD_DIR/CTFd/plugins/solve_webhook/config.py"
if [ $? -eq 0 ]; then
    echo "Webhook plugin config already exists."
else
    echo "Configuring webhook plugin. Running Ansible webhook_config task..."
    ansible-playbook -i "$INVENTORY" "$PLAYBOOK" --tags "webhook_config"
fi

echo "NervCTF setup complete!"

# Clean up temporary inventory file
rm "$INVENTORY"
