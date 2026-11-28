# os/provenance

`provenance.sh` makes the SBOM, the signed per-slot manifests, and the TEST keys for AllternitOS
(plan step 31). Full description: `os/README.md`, section "Provenance, Secure Boot,
reproducibility". Run by `os/tools/mkimage/mkimage.sh`; CI uploads `os/target/image/provenance/`
(public files only).

Never put a private key in this folder or anywhere in git. `.gitignore` here is a second net;
keys are generated into `os/target/image/keys/` per build.
