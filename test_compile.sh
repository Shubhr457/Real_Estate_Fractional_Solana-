#!/bin/bash

# Test script to check if the program compiles
echo "Testing compilation of Real Estate Fractional Solana program..."

cd /mnt/d/solana/Real_Estate_Fractional_Solana-

# Clean and build
echo "Running anchor build..."
anchor build

if [ $? -eq 0 ]; then
    echo "✅ Compilation successful!"
else
    echo "❌ Compilation failed!"
fi 