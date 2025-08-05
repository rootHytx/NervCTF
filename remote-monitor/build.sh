#!/bin/bash

# Build the remote-monitor Docker image
docker build -t remote-monitor .

# Build the ctfcli Docker image
docker build -f ctfcli.Dockerfile -t remote-monitor-ctfcli .

echo "Build complete. Images created:"
echo " - remote-monitor: Rust application image"
echo " - remote-monitor-ctfcli: ctfcli tool image"
