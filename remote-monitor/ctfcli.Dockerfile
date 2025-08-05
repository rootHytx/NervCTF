# Base image with Python and pip
FROM python:3.11-slim

# Install dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends git && \
    rm -rf /var/lib/apt/lists/*

# Install ctfcli
RUN pip install --no-cache-dir ctfcli

# Create working directory
WORKDIR /ctf

# Set default command to show help
ENTRYPOINT ["ctf"]
CMD ["--help"]
