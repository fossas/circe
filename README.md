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

# planned features

- [ ] Support extracting contents of OCI images:
  - [ ] From OCI stores
  - [ ] From local container hosts (e.g. Docker)
  - [ ] From local tarballs
- [ ] Extract the contents:
  - [ ] To disk
  - [ ] To stdout (as NDJSON)
- [ ] Extract layers by:
  - [ ] Squashed layer sets (e.g. "base + rest" or "all layers" or other combinations)
  - [ ] Individual layers
  - [ ] Filtered layers
- [ ] Specify target(s) to extract (e.g. `linux/amd64`, `darwin/arch64`, etc)
- [ ] Filter file(s) to extract
- [ ] When extracting files to stdout, store large blobs at temporary locations and reference them
