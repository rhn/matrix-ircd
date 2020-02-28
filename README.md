# Matrix IRCd

[![Build Status](https://travis-ci.org/matrix-org/matrix-ircd.svg?branch=master)](https://travis-ci.org/matrix-org/matrix-ircd)

An IRCd implementation backed by Matrix. Inspired by [PTO](https://github.com/tdfischer/pto)!

Join the discussion on the Matrix channel: [#matrix-ircd:matrix.org](https://matrix.to/#/#matrix-ircd:matrix.org)

This is project is almost the inverse of matrix-appservice-irc. **matrix-ircd** lets you use any standard IRC Client to communicate with Matrix, whereas **matrix-appservice-irc** is primarily a way to use a Matrix client to communicate with IRC.

## Status

**This is a work in progress.** Matrix IRCd should be stable enough to hack on
and test, but has not been tested in production or for any length of time.

See the GitHub issues page for a more detailed breakdown of what is left to do.


## Building

Matrix IRCd requires Rust v1.31.0 or later.
Building and installing uses the standard `cargo` commands.

To run a plain debug version:

```
cargo run -- --url "https://matrix.org"
```

To build with trace logging:

```
cargo build --features trace_logging
```


## Usage

```
IRC Matrix Daemon 0.1.0

USAGE:
    matrix-ircd [OPTIONS] --url <MATRIX_HS>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -b, --bind <BIND>            Sets the address to bind to. Defaults to 127.0.0.1:5999
        --url <MATRIX_HS>        The base url of the Matrix HS
        --password <PASSWORD>    The password of the PKCS#12 file
        --pkcs12 <PKCS12>        Sets the PKCS#12 file to read TLS cert and pkeyfrom

```

The `MATRIX_HS` URL should be of the form: `https://matrix.org`. Plain HTTP is
also supported but should *only* be used for testing and development.

Supplying both `pkcs12` and `password` arguments will cause Matrix IRCd to listen
on TLS, otherwise it will use plain TCP.

The credentials for the matrix account are taken from the user name and server
password specified by the IRC connection.


## Development

Matrix IRCd aims to build with zero standard warning and no clippy warnings.

To run clippy use (after install clippy):

```
cargo clippy --features clippy
```

The feature flag disables certain spurious warnings related to third party
crates.


Some high level development documentation can be generated by:

```
cargo doc --open
```

To generate the full documentation, including private APIs, use:

```
cargo rustdoc -- --no-defaults --passes "collapse-docs" --passes "unindent-comments" --open
```
