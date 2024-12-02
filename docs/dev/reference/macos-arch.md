# Reference: macOS binaries

Apple computers come in two flavors: Intel based and M-series based.

These correspond to two CPU architectures:
- M-series: `aarch64`
- Intel: `x86_64`

To build both, perform these steps in CI:
```shell
cross build --target=aarch64-apple-darwin --release
cross build --target=x86_64-apple-darwin --release
```

These steps may be done manually as well without using `cross`, if desired.
See [native cross compilation](./cross-compile.md#native-cross-compilation) for more information.
