#!/bin/bash
# Test script for update_test_config.py
# This script validates that the test configuration update script works correctly

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_SCRIPT="$PROJECT_ROOT/scripts/update_test_config.py"

echo "=== Testing update_test_config.py ==="
echo

# Test 1: Dry run with a hypothetical field
echo "Test 1: Dry run with hypothetical field"
python3 "$TEST_SCRIPT" --field test_hypothetical_field --value '"test_value"' --dry-run > /dev/null 2>&1
echo "✓ Dry run completed successfully"
echo

# Test 2: Check mode with existing field
echo "Test 2: Check mode with existing field (openai_api_key)"
python3 "$TEST_SCRIPT" --field openai_api_key --value '"test_key"' --check > /dev/null 2>&1
echo "✓ Check mode confirms openai_api_key is present in all configs"
echo

# Test 3: Help message
echo "Test 3: Help message"
python3 "$TEST_SCRIPT" --help > /dev/null 2>&1
echo "✓ Help message works"
echo

# Test 4: Test with missing required argument
echo "Test 4: Missing required argument handling"
if python3 "$TEST_SCRIPT" --field test_field 2>&1 | grep -q "required: --value"; then
    echo "✓ Correctly rejects missing --value argument"
else
    echo "✗ Failed to reject missing --value argument"
    exit 1
fi
echo

echo "=== All tests passed! ==="
echo
echo "The update_test_config.py script is working correctly."
echo
echo "Usage examples:"
echo "  python3 scripts/update_test_config.py --field new_field --value '\"default\"' --dry-run"
echo "  python3 scripts/update_test_config.py --field existing_field --value '\"test\"' --check"
