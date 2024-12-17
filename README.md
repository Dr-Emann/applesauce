# Applesauce

Applesauce is a command-line interface (CLI) program written in Rust that
compresses, decompresses, and prints information about compressed files for
HFS+/APFS transparent compression on macOS. It is based on
[afsctool](https://github.com/RJVB/afsctool) and offers several key
improvements, including better performance, improved multithreading (even for a
single file), and reduced memory usage. Applesauce supports all three compression
algorithms used by HFS+/APFS: LZFSE, LZVN, and ZLIB.

![compression example](https://github.com/user-attachments/assets/3f78a011-7db4-4e7f-9cc9-89781ed78609)

## Installation

### Install via Homebrew

To install Applesauce using Homebrew, run the following command:

```console
brew install Dr-Emann/homebrew-tap/applesauce
```

### Install prebuilt binaries via shell script

```console
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Dr-Emann/applesauce/releases/latest/download/applesauce-cli-installer.sh | sh
```

### Manual build with Cargo

<details>
<summary>Details</summary>

To install Applesauce using Cargo, follow these steps:

1. Install Rust and Cargo using the instructions provided at [rust-lang.org](https://www.rust-lang.org/tools/install).
2. Clone this repository to your local machine.
3. In the project directory, run `cargo build --release` to build the program.
4. The built binary can be found in the `target/release` directory.

</details>

## Usage

To use Applesauce, run the following command:

```console
applesauce [compress|decompress|info] file/directory
```

The options are as follows:

- `compress`: Compresses the specified file/directory using one of three compression algorithms (LZFSE, LZVN, or ZLIB).
- `decompress`: Decompresses the specified file/directory.
- `info`: Prints information about the specified compressed file/directory, including the compression ratio and
  compression algorithm used.

For example, to compress a file named `example.txt` using the ZLIB compression algorithm, you would run:

```console
applesauce compress -c ZLIB example.txt
```

## Features

Applesauce has the following key features:

- Supports three compression algorithms: LZFSE, LZVN, and ZLIB.
- Can print information about compressed files, including the compression ratio and compression algorithm used.
- Supports transparent compression for HFS+/APFS on macOS.

## Compression Algorithms

Applesauce supports three compression algorithms:

- LZFSE: This compression algorithm was developed by Apple for use on iOS and
  macOS. It is a fast compression algorithm that offers a good balance
  between compression ratio and speed.
- LZVN: This compression algorithm was also developed by Apple for use on iOS
  and macOS. It is optimized for use on 64-bit processors and offers a high
  compression ratio.
- ZLIB: This is a widely used compression algorithm that is implemented in many
  software packages. It is slower than LZFSE and LZVN, but can offer a higher
  compression ratio (especially with `-l 9`, but not always: it depends on the
  type of data being compressed).

Applesauce defaults to using LZFSE compression.
Depending on the type of data being compressed and the desired balance between
compression ratio and speed, one of these algorithms may be more suitable than
the others.

## Improvements Over Afsctool

Applesauce is based on afsctool, but offers several key improvements, including:

#### Improved Performance

afcstool can compress multiple files in parallel, but applesauce parallelizes at
the block level, so even a single file can be compressed in parallel. Applesauce
can often be several times faster than afsctool, especially for small numbers of
large files.

#### Reduced Memory Usage

afcstool will load the entire file into memory before compressing it
(although it does attempt to use mmap for large files). Applesauce will only
keep the block(s) currently being compressed in memory.

#### Better User Interface

Applesauce outputs a pretty progress bar while it's working, providing a more
user-friendly experience than afsctool's sparse output.

#### Better Compression With Many Small Files

afsctool will compress a file which fits in the xattr after compression, even
if doing so actually adds more overhead than leaving the file uncompressed.
Applesauce will not compress a file if it would result in a larger file.

#### Better Error Handling

afcstool overwrites files in place. Although it attempts to restore the file
if an error occurs, if it is forcefully terminated while compressing a file,
the file may be left in an invalid state.

Applesauce compresses/decompresses files to a temporary file and then atomically
renames the temporary file to the original file only when the operation is
complete: the file is never left in an invalid state, even if the program is
harshly terminated.

This is no replacement for backups: please do not use applesauce on files you
cannot afford to lose.

## License

Applesauce is licensed under the GNU General Public License version 3 (GPLv3).

## Contributions

Contributions to Applesauce are welcome! If you would like to contribute code,
please open a pull request on the GitHub repository. If you find a bug or have
a feature request, please open an issue on the repository.
