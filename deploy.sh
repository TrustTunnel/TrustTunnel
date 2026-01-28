#!/bin/bash
set -e

# TrustTunnel Deployment Script

echo ">>> Deploying TrustTunnel..."

# Check for .env
if [ ! -f .env ]; then
    echo "Error: .env file is missing. Run ./setup.sh or create it manually."
    exit 1
fi

# Pull latest changes (if in git repo)
if [ -d .git ]; then
    echo ">>> Pulling latest changes from git..."
    git pull
fi

# Build and Start
echo ">>> Building and starting containers..."
sudo docker compose up -d --build

echo ">>> Deployment successful!"
echo ">>> Checking logs..."
sudo docker compose logs -f trusttunnel
