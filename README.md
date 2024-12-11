# circe

_Circe (named after the Odyssean sorceress who transformed vessels and their contents) extracts and examines the contents of containers._

# usage

TBD, but generally the goal will be something like:

```shell
# Export the contents of the image to disk
; circe extract docker.io/contribsys/faktory:latest ./faktory --layers squash --platform linux/amd64

# Export the contents of the image to stdout as NDJSON
; circe read docker.io/contribsys/faktory:latest --layers squash --platform linux/amd64
```

# features

> [!NOTE]
> Unchecked features are vaguely planned but not implemented.

- [ ] Support extracting contents of OCI images:
  - [x] From OCI stores
  - [ ] From local container hosts (e.g. Docker)
  - [ ] From local tarballs
- [ ] Extract the contents:
  - [x] To disk
  - [ ] To stdout (as NDJSON)
- [ ] Extract layers by:
  - [x] Squashed layer sets (e.g. "base + rest" or "all layers" or other combinations)
  - [x] Individual layers
  - [ ] Filtered layers
- [x] Specify target(s) to extract (e.g. `linux/amd64`, `darwin/arch64`, etc)
- [ ] Filter file(s) to extract
- [ ] When extracting files to stdout, store large blobs at temporary locations and reference them
