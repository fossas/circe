# circe

<div align="center">

[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce.svg?type=shield&issueType=license)](https://app.fossa.com/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce?ref=badge_shield&issueType=license)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce.svg?type=shield&issueType=security)](https://app.fossa.com/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce?ref=badge_shield&issueType=security)

</div>


Circe (named after the Odyssean sorceress who transformed vessels and their contents)
extracts and examines the contents of containers.

# installation

> [!IMPORTANT]
> The installers below install to `~/.circe/bin`, or `%USERPROFILE%\circe\bin` on Windows.
> They try to place the binary in your PATH, but you may need to add it manually.

> [!TIP]
> You can update circe in the future with `circe-update`, which is installed alongside `circe`.

## macOS or Linux
```shell
curl -LsSf https://github.com/fossas/circe/releases/latest/download/circe-installer.sh | bash
```

## Windows

```shell
powershell -c "irm https://github.com/fossas/circe/releases/latest/download/circe-installer.ps1 | iex"
```

# usage

> [!TIP]
> Check the help output for more details.

```shell
# Export the contents of the image to disk.
#
# Usage:
#   circe extract <image> <target> [--layers <layers>] [--platform <platform>] [--overwrite]
#
# Arguments:
#   <image>
#       The image to extract.
#   <target>
#       The directory to which the image is extracted.
#
# Options for `circe extract`:
#   --layers
#       squash: Combines all layers into a single layer (default).
#       base: Excludes all layers except the base layer.
#       separate: Exports each layer in a separate subdirectory.
#   --platform
#       Defaults to your current platform.
#       Accepts the same values as `docker` (e.g. `linux/amd64`, `darwin/arm64`, etc).
#   --overwrite
#       If the target directory already exists, overwrite it.
#   --username
#       The username to use for authentication; "password" is also required if provided.
#   --password
#       The password to use for authentication; "username" is also required if provided.
circe extract docker.io/contribsys/faktory:latest ./faktory --layers squash --platform linux/amd64
```

## platform selection

You can customize the platform used by `circe` by passing `--platform`.

This is then used as follows:
- If the image is not multi-platform, this is ignored.
- If the image is multi-platform, this is used to select the platform to extract.
  - If the image does not publish the requested platform, `circe` reports this as an error.

If the image is multi-platform and no `--platform` argument is provided,
the first available platform is chosen according to the following priority list:

1. The first platform-independent image in the manifest
2. The current platform
3. The `linux` OS and the current architecture
4. The `linux` OS and the `amd64` architecture
5. The first image in the manifest

## layer selection

You can customize the layers extracted by `circe` by passing `--layers`.

The default is `squash`, which combines all layers into a single layer.

The other options are:
- `base`: Excludes all layers except the base layer.
- `separate`: Exports each layer in a separate subdirectory.

> [!TIP]
> The `separate` option also writes a `layers.json` file in the target directory,
> which is a JSON-encoded array of layer directory names.
> This array specifies the order of layer application in the image.

## troubleshooting

Set `RUST_LOG=debug` to get more detailed logs, and `RUST_LOG=trace` to get extremely detailed logs.
You can also filter to logs in a specific module (such as `circe` or `circe_lib`)
by setting `RUST_LOG=circe=debug` or `RUST_LOG=circe_lib=debug`.

> [!TIP]
> In macOS and Linux, you can apply environment variables to a command without changing your environment;
> for example: `RUST_LOG=trace circe ...`.

### future improvements

These are somewhat "known issues", but mostly "things to keep in mind" when using `circe`.
Ideally we'll fix these in the future; feel free to make a contribution or open an issue letting us know if one of these is blocking you.

- [ ] circe does not currently download layers concurrently.
  Since network transfer is effectively always the bottleneck, adding concurrent downloads would likely speed up `circe` significantly.
  That being said, as of our tests today `circe` is already about as fast as `docker pull && docker save`.
- [ ] symlinks are unpacked with the same destination as written in the actual container.
  This means e.g. they can link to files outside of the output directory
  (the example case I found was files in `usr/bin`, linking to `/bin/`).
