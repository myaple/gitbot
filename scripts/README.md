# GitBot Development Scripts

This directory contains utility scripts to help with GitBot development and testing.

## update_test_config.py

Automates updating test configurations when new fields are added to `AppSettings`.

### Features

- **Automatic field injection**: Finds all `AppSettings::default()` patterns in test files and adds new field assignments
- **Idempotent**: Skips files where the field already exists
- **Safe dry-run mode**: Preview changes before applying them
- **Check mode**: Verify a field exists in all test configurations
- **Auto-formatting**: Runs `cargo fmt` after making changes
- **Smart handling**: Properly wraps values in `Some()` for `Option<T>` fields when requested

### Usage

```bash
# Dry run to see what would change
python3 scripts/update_test_config.py --field new_field --value '"default_value"' --dry-run

# Actually add a new string field
python3 scripts/update_test_config.py --field new_field_name --value '"default_value"'

# Add a new numeric field
python3 scripts/update_test_config.py --field timeout_secs --value "120"

# Add an Option<T> field with None value
python3 scripts/update_test_config.py --field optional_field --value 'None' --rust-option

# Add an Option<T> field with Some(value)
python3 scripts/update_test_config.py --field cert_path --value '"/path/to/cert"' --rust-option

# Check if a field is present in all test configs
python3 scripts/update_test_config.py --field existing_field --value '"value"' --check
```

### Workflow for Adding New Configuration Fields

When you add a new field to `AppSettings` in `src/config.rs`:

1. Add the field to the `AppSettings` struct
2. Update the `Default` implementation with the default value
3. Run this script to update all test configurations
4. Run `cargo test` to verify everything works

Example:
```bash
# After adding a new field to AppSettings:
python3 scripts/update_test_config.py --field my_new_field --value '"default"'
cargo test
```

### Options

- `--field FIELD`: Name of the field to add (required)
- `--value VALUE`: Default value for the field (required)
- `--rust-option`: Wrap non-None values in `Some()` for `Option<T>` fields
- `--test-dir PATH`: Path to test directory (default: `src/tests`)
- `--dry-run`: Show what would change without modifying files
- `--check`: Verify field is present in all test configs (exit 1 if not)

## test_update_test_config.sh

Test suite for `update_test_config.py`. Validates that the script works correctly.

### Usage

```bash
./scripts/test_update_test_config.sh
```

This will run basic tests to ensure:
- Dry run mode works
- Check mode works
- Help message is available
- Required arguments are enforced
