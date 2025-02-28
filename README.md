# circe

<div align="center">

[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce.svg?type=shield&issueType=license)](https://app.fossa.com/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce?ref=badge_shield&issueType=license)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce.svg?type=shield&issueType=security)](https://app.fossa.com/projects/custom%2B1%2Fgithub.com%2Ffossas%2Fcirce?ref=badge_shield&issueType=security)

</div>


Circe (named after the Odyssean sorceress who transformed vessels and their contents)
extracts and examines the contents of containers.

_Looking for development docs? [Click here](./docs/dev/README.md)!_

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

## subcommand: extract

Extracts the contents of the image to disk.

```shell
# Extracts the contents of the image to disk.
#
# Usage:
#   circe extract <image> <target> [--layers <layers>] [--platform <platform>] [--overwrite]
#
# Arguments:
#   <image>
#       The image to extract. See image reference below for more details.
#   <target>
#       The directory to which the image is extracted.
#
# Options for `circe extract`:
#   --layers
#       squash: Combines all layers into a single layer (default).
#       squash-other: Combines all layers except the base layer into a single layer.
#       base: Excludes all layers except the base layer.
#       separate: Exports each layer in a separate subdirectory.
#   --platform
#       Defaults to your current platform.
#       Accepts the same values as `docker` (e.g. `linux/amd64`, `darwin/arm64`, etc).
#   --overwrite
#       If the target directory already exists, overwrite it.
#   --layer-glob, --lg
#       A glob pattern to filter layers to extract.
#       Layers matching this pattern are extracted.
#   --layer-regex, --lr
#       A regex pattern to filter layers to extract.
#       Layers matching this pattern are extracted.
#   --file-glob, --fg
#       A glob pattern to filter files to extract.
#       Files matching this pattern are extracted.
#   --file-regex, --fr
#       A regex pattern to filter files to extract.
#       Files matching this pattern are extracted.
#   --username
#       The username to use for authentication; "password" is also required if provided.
#   --password
#       The password to use for authentication; "username" is also required if provided.
circe extract docker.io/contribsys/faktory:latest ./faktory --layers squash --platform linux/amd64
```

## subcommand:list

Lists the contents of an image.

```shell
# Lists the contents of the image.
#
# Usage:
#   circe list <image> [--platform <platform>] [--username <username>] [--password <password>]
#
# Arguments:
#   <image>
#       The image to list. See image reference below for more details.
#
# Options for `circe list`:
#   --platform
#       Defaults to your current platform.
#       Accepts the same values as `docker` (e.g. `linux/amd64`, `darwin/arm64`, etc).
#   --username
#       The username to use for authentication; "password" is also required if provided.
#   --password
#       The password to use for authentication; "username" is also required if provided.
circe list docker.io/contribsys/faktory:latest
```

## image reference

The primary recommendation for referencing an image is to use the fully qualified reference, e.g.:

```shell
circe list docker.io/contribsys/faktory:latest
circe list docker.io/library/ubuntu:14.04
circe list some-host.dev/some-namespace/some-project/some-image:latest
circe list some-host.dev/some-namespace/some-project/some-image@sha256:123abc
```

Circe automatically checks your local Docker daemon first before pulling from a remote registry:

```shell
# Checks Docker daemon first for "alpine:latest" before pulling from registry
circe list alpine:latest 

# If the image exists locally, Circe will use it without making any network requests
circe extract nginx:latest ./nginx-extracted
```

However, for convenience, you can specify a "partial image reference" in a few different ways:

```shell
# namespace + name + tag; infers to docker.io/contribsys/faktory:latest
circe list contribsys/faktory:latest

# namespace + name + digest; infers to docker.io/contribsys/faktory@sha256:123abc
circe list contribsys/faktory@sha256:123abc

# namespace + name; infers to docker.io/contribsys/faktory:latest
circe list contribsys/faktory

# name + tag; infers to docker.io/library/ubuntu:latest
circe list ubuntu:latest

# name + digest; infers to docker.io/library/ubuntu@sha256:123abc
circe list ubuntu@sha256:123abc

# name; infers to docker.io/library/ubuntu:latest
circe list ubuntu
```

By default, `circe` fills in `docker.io` for the registry and `library` for the namespace.
However, you can customize the registry and namespace by setting the `OCI_BASE` and `OCI_NAMESPACE` environment variables:

```shell
# Specify the registry and/or namespace:
export OCI_BASE=some-host.dev
export OCI_NAMESPACE=some-namespace

# namespace + name + tag; infers to some-host.dev/contribsys/faktory:latest
circe list contribsys/faktory:latest

# namespace + name + digest; infers to some-host.dev/contribsys/faktory@sha256:123abc
circe list contribsys/faktory@sha256:123abc

# namespace + name; infers to some-host.dev/contribsys/faktory:latest
circe list contribsys/faktory

# name + tag; infers to some-host.dev/some-namespace/ubuntu:latest
circe list ubuntu:latest

# name + digest; infers to some-host.dev/some-namespace/ubuntu@sha256:123abc
circe list ubuntu@sha256:123abc

# name; infers to some-host.dev/some-namespace/ubuntu:latest
circe list ubuntu
```

**The overall recommendation is to use fully qualified references.**
The intention with the ability to override `OCI_BASE` and `OCI_NAMESPACE` is to make setup easier for CI/CD pipelines
that need to extract multiple images from a custom host and/or namespace, but don't want to have to write scripts
to concatenate them into fully qualified references.

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

#### future improvements

These are somewhat "known issues", but mostly "things to keep in mind" when using `circe`.
Ideally we'll fix these in the future; feel free to make a contribution or open an issue letting us know if one of these is blocking you.

- [ ] circe does not currently download layers concurrently.
  Since network transfer is effectively always the bottleneck, adding concurrent downloads would likely speed up `circe` significantly.
  That being said, as of our tests today `circe` is already about as fast as `docker pull && docker save`.
