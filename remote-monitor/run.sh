# Check if Docker is running
if ! docker info >/dev/null 2>&1; then
    echo "Error: Docker daemon is not running"
    exit 1
fi

# Set CTFd configuration
export CTFD_URL=${CTFD_URL:-"http://localhost:8000"}
export CTFD_API_KEY=${CTFD_API_KEY:-"your-api-key-here"}

# Build images if not already built
if ! docker image inspect remote-monitor >/dev/null 2>&1; then
    echo "Building remote-monitor image..."
    docker build -t remote-monitor .
fi

if ! docker image inspect remote-monitor-ctfcli >/dev/null 2>&1; then
    echo "Building remote-monitor-ctfcli image..."
    docker build -f ctfcli.Dockerfile -t remote-monitor-ctfcli .
fi

# Run the configurator
echo "Running CTFd Configurator..."
echo "CTFd URL: $CTFD_URL"
docker run -it --rm \
    -v "$(pwd)/challenges:/ctf/challenges" \
    -e CTFD_URL \
    -e CTFD_API_KEY \
    remote-monitor "$@"
