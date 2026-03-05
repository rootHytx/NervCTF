ENV_FILE="./.env"
persist_vars=("TARGET_IP" "TARGET_USER" "SSH_PUBKEY_PATH" "CTFD_PATH")

echo "=============================================="
echo " NervCTF Setup: Automated CTFd Environment"
echo "----------------------------------------------"
echo "This script will:"
echo " - Install Rootless Docker, CTFd, the solve webhook plugin, and the Remote Monitor"
echo " - Configure SSH access for the deployment user"
echo
echo "=============================================="
echo

# Source .env if it exists
if [[ -f "$ENV_FILE" ]]; then
    export $(grep -v '^#' "$ENV_FILE" | xargs)
else
    echo "No .env file found in the current directory."
    echo "Creating a new .env file."
    touch "$ENV_FILE"
fi

missing_vars=()
for var in "${persist_vars[@]}"; do
    if [[ -z "${!var}" ]]; then
        missing_vars+=("$var")
    fi
done

persist_all=""
if [[ ${#missing_vars[@]} -eq 0 ]]; then
    echo "All required environment variables are present. Proceeding with setup using current configuration..."
    all_in_env=true
    persist_all="n"
else
    all_in_env=false
fi

# Only ask about persisting if not all are present
if ! $all_in_env; then
    read -p "Do you want to persist all variables you enter to $ENV_FILE for future runs? (y/n): " persist_all
fi

# Only prompt for TARGET_IP if not already set
if [ -z "$TARGET_IP" ]; then
    read -p "Enter target machine IP address: " TARGET_IP
else
    echo "Using existing TARGET_IP: $TARGET_IP"
fi
[ -z "$TARGET_IP" ] && { echo "IP address required"; exit 1; }

# Only prompt for TARGET_USER if not already set
if [ -z "$TARGET_USER" ]; then
    read -p "Remote sudo user (default: root): " TARGET_USER
    TARGET_USER=${TARGET_USER:-root}
else
    echo "Using existing TARGET_USER: $TARGET_USER"
fi

TARGET_HOST="$TARGET_USER@$TARGET_IP"

# Only prompt for CTFD_PATH if not already set
if [ -z "$CTFD_PATH" ]; then
    read -p "Is CTFd already installed? (y/n): " CTFD_INSTALLED
    CTFD_INSTALLED=${CTFD_INSTALLED,,}  # convert to lowercase
    if [[ $CTFD_INSTALLED == "y" || $CTFD_INSTALLED == "yes" ]]; then
        read -p "Enter the full path to the CTFd directory: " CTFD_PATH
    fi
else
    echo "Using existing CTFD_PATH: $CTFD_PATH"
fi

CTFD_PATH="${CTFD_PATH:-}"

# SSH key selection
if [ -z "$SSH_PUBKEY_PATH" ]; then
    echo
    echo "Available SSH public keys in ~/.ssh:"
    pubkeys=()
    i=1
    for keyfile in ~/.ssh/*.pub; do
        [ -e "$keyfile" ] || continue
        echo "  [$i] $keyfile"
        pubkeys+=("$keyfile")
        i=$((i+1))
    done

    if [ ${#pubkeys[@]} -eq 0 ]; then
        echo "No existing SSH public keys found in ~/.ssh."
        read -p "Do you want to generate a new SSH key? (y/n): " generate
        generate=${generate,,}  # convert to lowercase
        if [[ "$generate" =~ ^[Yy]$ ]]; then
            ssh-keygen -t rsa -b 4096 -f ~/.ssh/nervctf_ansible_id_rsa -N ""
            SSH_PUBKEY_PATH="$HOME/.ssh/nervctf_ansible_id_rsa.pub"
        else
            echo "No SSH key selected. Exiting."
            exit 1
        fi
    else
        read -p "Enter the number of the key to use, or type 'new' to generate a new key: " key_choice
        if [[ "$key_choice" == "new" ]]; then
            ssh-keygen -t rsa -b 4096 -f ~/.ssh/nervctf_ansible_id_rsa -N ""
            SSH_PUBKEY_PATH="$HOME/.ssh/nervctf_ansible_id_rsa.pub"
        elif [[ "$key_choice" =~ ^[0-9]+$ ]] && [ "$key_choice" -ge 1 ] && [ "$key_choice" -le "${#pubkeys[@]}" ]; then
            SSH_PUBKEY_PATH="${pubkeys[$((key_choice-1))]}"
        else
            echo "Invalid choice. Exiting."
            exit 1
        fi
    fi
else
    echo "Using existing SSH public key: $SSH_PUBKEY_PATH"
fi

# Persist variables if user agreed
if [[ "$persist_all" =~ ^[Yy]$ ]]; then
    # Only add if not already present
    for var in "${missing_vars[@]}"; do
        echo "$var=${!var}" >> "$ENV_FILE"
    done
    echo "Variable(s) persisted to $ENV_FILE."
fi

INVENTORY=$(mktemp)
cat <<EOF > "$INVENTORY"
[ctfd]
$TARGET_IP ansible_user=$TARGET_USER
EOF

extra_vars="ssh_key=$SSH_PUBKEY_PATH ctfd_path=$CTFD_PATH"

ansible-playbook -i "$INVENTORY" ./utils/nervctf_playbook.yml --extra-vars "$extra_vars"

rm "$INVENTORY"
echo "NervCTF setup complete!"
