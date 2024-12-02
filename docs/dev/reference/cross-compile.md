# Reference: Cross compilation

You're working with a macOS or Linux system, and everything is going great.

Satisfied with your work, you push a PR, and sit back to get a cup of coff-
wait, what's that Windows build issue?

```
error[E0793]: reference to packed field is unaligned
    --> C:\Users\runneradmin\.cargo\registry\src\index.crates.io-6f17d22bba15001f\ntapi-0.3.7\src\ntexapi.rs:2783:52
     |
2783 |         *tick_count.QuadPart_mut() = read_volatile(&(*USER_SHARED_DATA).u.TickCountQuad);
     |                                                    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
     |
     = note: fields of packed structs are not properly aligned, and creating a misaligned reference is undefined behavior (even if that reference is never dereferenced)
     = help: copy the field contents to a local variable, or replace the reference with a raw pointer and use `read_unaligned`/`write_unaligned` (loads and stores via `*p` must be properly aligned even when using raw pointers)
```

You then spend several PR cycles trying to get this to be fixed, each time with a delay 
(and a bunch of wasteful parallel CI jobs running, whose results you don't care about).

Wouldn't it be better if instead, you could just build for Windows locally?

## The better way

You can! ✨

Steps:

1. Install [`cross`](https://github.com/cross-rs/cross): `cargo binstall cross`
1. `cross build --target x86_64-pc-windows-gnu`

Now your build still fails but at least your testing loop is faster 🥲

## Cross version compile

You can also do this across versions:

```
; rustup install 1.68
; rustup run 1.68 cross build --target x86_64-pc-windows-gnu
```

## Native cross compilation

`cross` uses Docker to do its thing.

If you don't like that, you can cross compile by installing dependencies yourself,
assuming that's supported without emulation:

1. Install the `mingw-64` package.
  - On macOS, that's `brew install mingw-w64`
  - On Debian/Ubuntu, that's `sudo apt-get install mingw-w64`
  - On Arch, that's `sudo pacman -S mingw-w64`, install all
1. `rustup target add x86_64-pc-windows-gnu`
1. `cargo build --target x86_64-pc-windows-gnu`

And that still works across versions:

```
; rustup install 1.68
; rustup target add x86_64-pc-windows-gnu --toolchain 1.68
; rustup run 1.68 cargo build --target x86_64-pc-windows-gnu
```
