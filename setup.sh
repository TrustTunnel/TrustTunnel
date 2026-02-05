#!/bin/bash
set -e

# TrustTunnel Server Setup Script
# This script installs Docker and Docker Compose on a fresh Linux server.

echo ">>> Starting TrustTunnel Server Setup..."

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    echo ">>> Docker not found. Installing Docker..."
    
    # Update and install prereqs
    sudo apt-get update
    sudo apt-get install -y ca-certificates curl gnupg

    # Add Docker's official GPG key
    sudo install -m 0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/debian/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    sudo chmod a+r /etc/apt/keyrings/docker.gpg

    # Set up repository
    echo \
      "deb [arch=\"$(dpkg --print-architecture)\" signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/debian \
      $(. /etc/os-release && echo \"$VERSION_CODENAME\") stable" | \
      sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

    # Install Docker Engine
    sudo apt-get update
    sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
    
    echo ">>> Docker installed successfully."
else
    echo ">>> Docker is already installed."
fi

# Setup Environment
if [ ! -f .env ]; then
    echo ">>> .env file not found."
    if [ -f .env.example ]; then
        echo ">>> Copying .env.example to .env..."
        cp .env.example .env
        echo ">>> Please edit .env with your configuration:"
        echo "    nano .env"
    else
        echo ">>> Warning: .env.example not found. Please create .env manually."
    fi
else
    echo ">>> .env file exists."
fi

echo ">>> Setup complete!"
echo ">>> Run './deploy.sh' to start the service."
