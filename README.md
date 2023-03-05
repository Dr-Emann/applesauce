# Applesauce

Applesauce is a command-line interface (CLI) program written in Rust that
compresses, decompresses, and prints information about compressed files for
HFS+/APFS transparent compression on macOS. It is based on
[afsctool](https://github.com/RJVB/afsctool) and offers several key
improvements, including better performance, improved multithreading (even for a
single file), reduced memory usage, and is written in Rust. Applesauce supports
three compression algorithms: LZFSE, LZVN, and ZLIB.

## Installation

### Building with Cargo

To install Applesauce using Cargo, follow these steps:

1. Install Rust and Cargo using the instructions provided at [rust-lang.org](https://www.rust-lang.org/tools/install).
2. Clone this repository to your local machine.
3. In the project directory, run `cargo build --release` to build the program.
4. The built binary can be found in the `target/release` directory.

### Installing from GitHub Releases

Alternatively, you can download pre-built binaries from the [GitHub releases page](https://github.com/Dr-Emann/applesauce/releases).

## Usage

To use Applesauce, run the following command:

```console
applesauce [compress|decompress|info] file
```


The options are as follows:

- `compress`: Compresses the specified file using one of three compression algorithms (LZFSE, LZVN, or ZLIB).
- `decompress`: Decompresses the specified file.
- `info`: Prints information about the specified compressed file, including the compression ratio and compression algorithm used.

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
  macOS. It is a very fast compression algorithm that offers a good balance
  between compression ratio and speed. LZFSE is particularly good at
  compressing data that has lots of repeated patterns.
- LZVN: This compression algorithm was also developed by Apple for use on iOS
  and macOS. It is optimized for use on 64-bit processors and offers a high
  compression ratio. LZVN is particularly good at compressing image and video
  data.
- ZLIB: This is a widely used compression algorithm that is implemented in many
  different software packages. It is slower than LZFSE and LZVN but offers a
  higher compression ratio. ZLIB is particularly good at compressing text data.

Applesauce defaults to using LZFSE compression.
Depending on the type of data being compressed and the desired balance between
compression ratio and speed, one of these algorithms may be more suitable than
the others.

## Improvements Over Afsctool

Applesauce is based on afsctool, but offers several key improvements, including:

- Better performance
- Improved multithreading
- Reduced memory usage
- Written in Rust

## License
Applesauce is licensed under the GNU General Public License version 3 (GPLv3).

## Contributions
Contributions to Applesauce are welcome! If you would like to contribute code,
please open a pull request on the GitHub repository. If you find a bug or have
a feature request, please open an issue on the repository.
