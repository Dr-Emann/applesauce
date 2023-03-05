# Applesauce

Applesauce is a transparent file compression tool for macos.
It is a command line utility that can be used to compress files with HFS+/APFS built-in compression support.
It is based on [afsctool] (originally written by brkirch, forked and maintained by RJVB).

It is rewritten in Rust to take better advantage of multithreading (especially for large files).

[afsctool]: https://github.com/RJVB/afsctool