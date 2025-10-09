#!/bin/bash

compressible_file() {
  local size="${2:-1000}"
  printf "%${size}s" '' > "$1"
  echo "$1"
}

# Function to create deeply nested folders
create_deep_folders() {
  local deep_path="$1"
  for ((i=0; i<50; i++)); do
    deep_path="$deep_path/deep$i"
  done
  mkdir -p "$deep_path"
  compressible_file "$deep_path/file"
}

# Function to create files with various properties
create_files() {
  local target="$1"

  touch "$target/empty"
  compressible_file "$target/small"
  compressible_file "$target/big" $((100 * 1000 * 1000))

  # Large files that won't compress well (e.g., binary data)
  head -c $((100 * 1000 * 1000)) /dev/urandom > "$target/big_random"

  compressible_file "$target/link1"
  # hardlink
  ln "$target/link1" "$target/link2"

  ln -s "/tmp" "$target/symlink_outside"

  ln -s "does not exist" "$target/broken_symlink"

  compressible_file "$target/has_symlink"
  ln -s "has_symlink" "$target/symlink_to_has_symlink"

  mkfifo "$target/fifo"

  mkdir -p "$target/small_files"
  create_many_files "$target/small_files"
}

create_many_files() {
  local target="$1"

  local pids=()
  for i in {1..10}; do
    for j in {1..1000}; do
      compressible_file "$target/$((i * 1000 + j))" 10 &
      pids+=($!)
    done
    wait "${pids[@]}"
    pids=()
  done
}

# Main function to setup the folder structure
setup_complex_folder() {
  # Call main function with passed argument
  if [ $# -eq 0 ]; then
    echo "Usage: $0 <directory>" >&2
    exit 2
  fi
  local target="$1"
  if ! [ -d "$target" ]; then
    echo "Directory '$target' does not exist" >&2
    exit 1
  fi

  create_deep_folders "$target"
  create_files "$target"

  echo "Complex folder setup completed."
}


setup_complex_folder "$@"
