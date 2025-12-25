#!/usr/bin/env python3
"""
Script to update test configurations when new fields are added to AppSettings.

This script finds all locations in test files where AppSettings is configured
and adds specified field assignments to them. This prevents having to manually
update each test when new configuration fields are added.

Usage:
    python scripts/update_test_config.py --field new_field_name --value "default_value"
    python scripts/update_test_config.py --field openai_timeout_secs --value "120"
    python scripts/update_test_config.py --field prompt_prefix --value 'None' --rust-option

Examples:
    # Add a simple string field
    python scripts/update_test_config.py --field openai_model --value '"gpt-3.5-turbo"'

    # Add a numeric field
    python scripts/update_test_config.py --field openai_timeout_secs --value "120"

    # Add an Option field with None value
    python scripts/update_test_config.py --field prompt_prefix --value 'None' --rust-option

    # Add an Option field with Some(value)
    python scripts/update_test_config.py --field client_cert_path --value '"/path/to/cert"' --rust-option
"""

import argparse
import re
import sys
from pathlib import Path
from typing import List, Tuple, Optional


def find_test_files(test_dir: Path) -> List[Path]:
    """Find all Rust test files in the test directory."""
    return list(test_dir.rglob("*.rs"))


def find_and_update_configurations(
    content: str, field_name: str, field_value: str, is_option: bool
) -> Tuple[str, int]:
    """
    Find and update all AppSettings configurations in the file content.

    Returns:
        Tuple of (updated_content, number_of_changes)
    """
    lines = content.split('\n')
    updated_lines = []
    i = 0
    changes = 0

    while i < len(lines):
        line = lines[i]
        updated_lines.append(line)

        # Look for AppSettings::default() declarations
        match = re.search(r'let\s+(mut\s+)?(\w+)\s*=\s*AppSettings::default\(\)', line)
        if match:
            var_name = match.group(2)

            # Collect subsequent field assignment lines
            j = i + 1
            assignment_lines = []

            while j < len(lines):
                next_line = lines[j]

                # Stop conditions: another let, function, test attribute, or empty line followed by non-assignment
                if re.match(r'\s*(let |fn |#\[|$)', next_line):
                    break

                # Check if this line has a field assignment to our variable
                assignment_match = re.search(
                    rf'\b{var_name}\s*\.\s*(\w+)\s*=', next_line
                )
                if assignment_match:
                    # Check if this is the field we're trying to add
                    if assignment_match.group(1) == field_name:
                        # Field already exists, skip adding it
                        assignment_lines = []
                        j = i + 1  # Reset to continue after the declaration
                        break

                    assignment_lines.append((j, next_line))
                    j += 1
                else:
                    # Not a field assignment to our variable
                    break

            # If we found assignment lines and the field doesn't exist, add it
            if assignment_lines:
                # Determine indentation from the last assignment line
                last_assignment_line = assignment_lines[-1][1]
                indent_match = re.match(r'(\s*)', last_assignment_line)
                base_indent = indent_match.group(1) if indent_match else ''

                # Prepare the value
                value_to_insert = field_value
                if is_option and field_value != 'None':
                    value_to_insert = f'Some({field_value})'

                # Create the new field assignment
                new_field_line = f'{base_indent}{var_name}.{field_name} = {value_to_insert};'

                # Insert after the last assignment line
                insert_position = assignment_lines[-1][0] + 1

                # Update lines list by inserting the new line
                for idx, _ in assignment_lines:
                    updated_lines.append(lines[idx])

                updated_lines.append(new_field_line)
                changes += 1

                # Move i to after the insert position
                i = insert_position - 1  # -1 because we'll increment at the end
            else:
                # No assignment lines found, or field already exists
                # Just continue, we've already added the current line
                pass

        i += 1

    return '\n'.join(updated_lines), changes


def process_file(
    file_path: Path, field_name: str, field_value: str, is_option: bool, dry_run: bool = False
) -> bool:
    """
    Process a single file to add the new field.

    Returns:
        True if file was modified, False otherwise
    """
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            original_content = f.read()

        updated_content, changes = find_and_update_configurations(
            original_content, field_name, field_value, is_option
        )

        if changes > 0:
            if dry_run:
                print(f"Would modify: {file_path} ({changes} changes)")
                # Show what changed
                orig_lines = original_content.split('\n')
                updated_lines = updated_content.split('\n')
                for i, (orig, upd) in enumerate(zip(orig_lines, updated_lines)):
                    if orig != upd:
                        print(f"  Line {i+1}:")
                        print(f"    - {orig}")
                        print(f"    + {upd}")
                        # Show a few lines of context
                        for j in range(i+1, min(i+3, len(updated_lines))):
                            if j < len(orig_lines) and j < len(updated_lines):
                                if orig_lines[j] != updated_lines[j]:
                                    print(f"    Line {j+1}: {updated_lines[j]}")
                        break
            else:
                with open(file_path, 'w', encoding='utf-8') as f:
                    f.write(updated_content)
                print(f"Updated: {file_path} ({changes} changes)")

            return True
        else:
            return False

    except Exception as e:
        print(f"Error processing {file_path}: {e}", file=sys.stderr)
        import traceback
        traceback.print_exc()
        return False


def check_field_presence(test_files: List[Path], field_name: str) -> bool:
    """
    Check if the field is present in all AppSettings configurations.

    Returns:
        True if field is present in all configs, False otherwise
    """
    all_present = True

    for test_file in test_files:
        try:
            with open(test_file, 'r') as f:
                content = f.read()

            # Find all AppSettings::default() patterns
            default_matches = re.finditer(r'let\s+(mut\s+)?(\w+)\s*=\s*AppSettings::default\(\)', content)

            for match in default_matches:
                var_name = match.group(2)

                # Look ahead for field assignments
                start_pos = match.end()
                remaining_content = content[start_pos:]

                # Extract the block of assignments (until next let, fn, #[, or end)
                lines_after = remaining_content.split('\n')
                assignment_block = []
                for line in lines_after:
                    if re.match(r'\s*(let |fn |#\[|$)', line):
                        break
                    assignment_block.append(line)

                # Check if our field is in the assignment block
                block_text = '\n'.join(assignment_block)
                if f'{var_name}.{field_name}' not in block_text:
                    print(f"Missing field '{field_name}' in {test_file}")
                    all_present = False

        except Exception as e:
            print(f"Error checking {test_file}: {e}", file=sys.stderr)
            all_present = False

    return all_present


def main():
    parser = argparse.ArgumentParser(
        description='Update test configurations when new fields are added to AppSettings'
    )
    parser.add_argument(
        '--field', required=True, help='Name of the field to add (e.g., openai_timeout_secs)'
    )
    parser.add_argument(
        '--value', required=True, help='Default value for the field (e.g., "120", "None")'
    )
    parser.add_argument(
        '--rust-option',
        action='store_true',
        help='If set, wraps non-None values in Some() for Option<T> fields'
    )
    parser.add_argument(
        '--test-dir',
        default='src/tests',
        help='Path to test directory (default: src/tests)'
    )
    parser.add_argument(
        '--dry-run',
        action='store_true',
        help='Show what would be changed without actually modifying files'
    )
    parser.add_argument(
        '--check',
        action='store_true',
        help='Check if the field is already present in all test configs (exit 1 if not)'
    )

    args = parser.parse_args()

    test_dir = Path(args.test_dir)

    if not test_dir.exists():
        print(f"Error: Test directory {test_dir} does not exist", file=sys.stderr)
        sys.exit(1)

    test_files = find_test_files(test_dir)

    if not test_files:
        print(f"No test files found in {test_dir}")
        sys.exit(0)

    print(f"Found {len(test_files)} test files")

    if args.check:
        # Check mode: verify field exists in all configs
        print(f"Checking for field '{args.field}' in all test configurations...")
        if check_field_presence(test_files, args.field):
            print(f"✓ Field '{args.field}' is present in all test configurations")
            sys.exit(0)
        else:
            print(f"\n✗ Field '{args.field}' is missing in one or more test configurations")
            sys.exit(1)

    # Update mode
    modified_count = 0
    for test_file in test_files:
        if process_file(test_file, args.field, args.value, args.rust_option, args.dry_run):
            modified_count += 1

    if args.dry_run:
        print(f"\nWould modify {modified_count} files")
    else:
        print(f"\nModified {modified_count} files")

        if modified_count > 0:
            # Run cargo fmt to ensure proper formatting
            print("\nRunning 'cargo fmt' to format updated files...")
            import subprocess
            result = subprocess.run(
                ['cargo', 'fmt'],
                cwd=test_dir.parent.parent,
                capture_output=True,
                text=True
            )
            if result.returncode == 0:
                print("✓ Formatting complete!")
            else:
                print("Warning: cargo fmt failed")
                print(result.stderr)


if __name__ == '__main__':
    main()
