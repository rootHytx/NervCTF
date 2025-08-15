#!/bin/bash

# Test script for ChallengeManager
# Creates a temporary challenge directory structure and tests operations

set -e

# Create temporary directory for test challenges
TEST_DIR=$(mktemp -d)
echo "Created test directory: $TEST_DIR"

# Create sample challenges
mkdir -p "$TEST_DIR/category1/challenge1"
cat > "$TEST_DIR/category1/challenge1/challenge.yml" <<EOF
name: Challenge 1
category: Category 1
description: First test challenge
value: 100
flags:
  - flag{test1}
files: []
EOF

mkdir -p "$TEST_DIR/category1/challenge2"
cat > "$TEST_DIR/category1/challenge2/challenge.yml" <<EOF
name: Challenge 2
category: Category 1
description: Second test challenge
value: 200
flags:
  - flag{test2}
files: []
EOF

mkdir -p "$TEST_DIR/category2/challenge3"
cat > "$TEST_DIR/category2/challenge3/challenge.yml" <<EOF
name: Challenge 3
category: Category 2
description: Third test challenge
value: 300
flags:
  - flag{test3}
files: []
EOF

# Build the application
echo "Building application..."
cargo build --release

# Test operations
echo -e "\nTesting sync operation:"
./target/release/local-configurator challenges sync --path "$TEST_DIR"

echo -e "\nTesting lint operation:"
./target/release/local-configurator challenges lint --path "$TEST_DIR"

echo -e "\nTesting verify operation:"
./target/release/local-configurator challenges verify --path "$TEST_DIR"

# Cleanup
echo -e "\nCleaning up test directory"
rm -rf "$TEST_DIR"

echo -e "\nAll tests completed successfully!"
