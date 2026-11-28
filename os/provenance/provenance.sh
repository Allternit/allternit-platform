#!/bin/sh
# AllternitOS provenance tooling (plan step 31). POSIX sh + openssl (3.x), jq, sha256sum, cargo.
#
#   provenance.sh keygen KEYDIR
#       TEST keys, fresh: Ed25519 update-signing key + Secure Boot PK/KEK/db (RSA-2048 certs).
#       Private files stay in KEYDIR (never committed, never uploaded); KEYDIR/public/ holds
#       the public halves (certs, update-pub.pem, update-pub.key = raw 32 bytes).
#   provenance.sh sbom OUT.json --os-dir DIR --bin-dir DIR [--target vm|pc] [--kconfig F] [--llama F] [--model F] [--firmware DIR]
#       SPDX 2.3 JSON: kernel (version, tarball sha256, config hash), llama.cpp tag + commit,
#       the GGUF sha256, each Rust binary (sha256) with its resolved crates from Cargo.lock
#       (`cargo tree` per binary, so only crates that binary links).
#       --target pc hashes x86_64-pc.config. --firmware DIR (os/image/firmware/fetch.sh output)
#       adds the linux-firmware tag + tarball sha256 and one package per group (iwlwifi, mt7921,
#       r8169, i915, amdgpu): checksum = sha256 of the group's sorted "sha256 path" lines, the
#       file count in the comment, and the licence as a LicenseRef with its text and sha256 in
#       hasExtractedLicensingInfos.
#   provenance.sh manifest OUTDIR KEY.pem VERSION UKI SBOM  A-VERITY A-SHA A-BYTES  B-VERITY B-SHA B-BYTES
#       manifest-a.json/.sig and manifest-b.json/.sig. VERITY is root:salt:blocks. The JSON is
#       compact with sorted keys; the Ed25519 signature (raw, 64 bytes) covers those exact bytes.
#   provenance.sh verify PUB.pem MANIFEST.json SIG
#       exit 0 when the signature is good.
#
#   provenance.sh retarget SRC.json KEY.pem OUTPREFIX SHA BYTES
#       CI only: a validly signed copy of a manifest that describes a different image
#       (OUTPREFIX.json/.sig), for the bad-update negative case.
#   provenance.sh enroll VARS_IN VARS_OUT KEYDIR
#       OVMF vars with PK/KEK/db enrolled from the TEST certs and Secure Boot on (virt-fw-vars).
#
# Nothing here reads a secret from the environment or the repo. Keys come from keygen, which
# CI runs fresh in every run. Real release keys are a later decision (TODO: GitHub Actions
# secret or an offline signer, with the public key replacing update-pub.key at image build).
set -eu

die() { echo "provenance: $*" >&2; exit 1; }
say() { echo "provenance: $*"; }
sha() { sha256sum "$1" | cut -d' ' -f1; }

cmd=${1:-}; [ $# -gt 0 ] && shift

case "$cmd" in
keygen)
  KD=${1:?keygen KEYDIR}
  umask 077
  mkdir -p "$KD/public"
  openssl genpkey -algorithm ED25519 -out "$KD/update-sign.pem" 2>/dev/null
  openssl pkey -in "$KD/update-sign.pem" -pubout -out "$KD/public/update-pub.pem"
  # Raw public key: the last 32 bytes of the 44-byte SubjectPublicKeyInfo DER.
  openssl pkey -in "$KD/update-sign.pem" -pubout -outform DER | tail -c 32 > "$KD/public/update-pub.key"
  [ "$(wc -c < "$KD/public/update-pub.key")" -eq 32 ] || die "raw public key is not 32 bytes"
  for n in PK KEK db; do
    openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
      -keyout "$KD/$n.key" -out "$KD/public/$n.crt" \
      -subj "/CN=Allternit TEST $n - do not trust/O=Allternit TEST KEYS" 2>/dev/null
  done
  cat /proc/sys/kernel/random/uuid > "$KD/owner.guid" 2>/dev/null || uuidgen > "$KD/owner.guid"
  cp "$KD/owner.guid" "$KD/public/owner.guid"
  printf 'TEST keys made for one build. They protect nothing and must never be trusted by real hardware.\n' > "$KD/public/TEST-KEYS.txt"
  say "TEST keys in $KD (private halves stay there; public halves in $KD/public)"
  ;;

sbom)
  OUTF=${1:?sbom OUT.json}; shift
  OS_DIR=""; BIN_DIR=""; KCONF=""; LLAMA=""; MODEL=""; ITARGET=vm; FWDIR=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --os-dir) OS_DIR=$2; shift 2 ;; --bin-dir) BIN_DIR=$2; shift 2 ;;
      --target) ITARGET=$2; shift 2 ;; --firmware) FWDIR=$2; shift 2 ;;
      --kconfig) KCONF=$2; shift 2 ;; --llama) LLAMA=$2; shift 2 ;; --model) MODEL=$2; shift 2 ;;
      *) die "sbom: unknown flag $1" ;;
    esac
  done
  [ -n "$OS_DIR" ] && [ -n "$BIN_DIR" ] || die "sbom needs --os-dir and --bin-dir"
  SDE=${SOURCE_DATE_EPOCH:-0}
  CREATED=$(date -u -d "@$SDE" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -r "$SDE" +%Y-%m-%dT%H:%M:%SZ)
  COMMIT=$(git -C "$OS_DIR" rev-parse HEAD 2>/dev/null || echo unknown)
  TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
  : > "$TMP/pkgs"; : > "$TMP/rels"
  # pkg ID NAME VERSION SHA256|- PURL|- COMMENT  -> one SPDX package per line
  pkg() {
    jq -nc --arg id "$1" --arg name "$2" --arg ver "$3" --arg sum "$4" --arg purl "$5" --arg c "$6" '
      {SPDXID:$id, name:$name, versionInfo:$ver, downloadLocation:"NOASSERTION", filesAnalyzed:false,
       licenseConcluded:"NOASSERTION", licenseDeclared:"NOASSERTION", copyrightText:"NOASSERTION"}
      + (if $sum != "-" then {checksums:[{algorithm:"SHA256", checksumValue:$sum}]} else {} end)
      + (if $purl != "-" then {externalRefs:[{referenceCategory:"PACKAGE-MANAGER", referenceType:"purl", referenceLocator:$purl}]} else {} end)
      + (if $c != "" then {comment:$c} else {} end)' >> "$TMP/pkgs"
  }
  rel() { jq -nc --arg a "$1" --arg t "$2" --arg b "$3" '{spdxElementId:$a, relationshipType:$t, relatedSpdxElement:$b}' >> "$TMP/rels"; }

  . "$OS_DIR/image/kernel/VERSION"
  KCFG_SHA=$(sha "$OS_DIR/image/kernel/x86_64-$ITARGET.config")
  KC="config fragment sha256 $KCFG_SHA"
  [ -z "$KCONF" ] || [ ! -f "$KCONF" ] || KC="$KC; final .config sha256 $(sha "$KCONF")"
  pkg SPDXRef-kernel linux-kernel "$KERNEL_VERSION" "$KERNEL_SHA256" "-" "kernel.org tarball sha256; $KC"
  rel SPDXRef-DOCUMENT DESCRIBES SPDXRef-kernel

  : > "$TMP/lics"
  if [ -n "$FWDIR" ]; then
    [ -f "$FWDIR/firmware.list" ] && [ -f "$FWDIR/groups.tsv" ] || die "sbom: $FWDIR has no firmware.list / groups.tsv"
    ( . "$OS_DIR/image/firmware/LINUX_FIRMWARE"
      pkg SPDXRef-linux-firmware linux-firmware "$FW_TAG" "$FW_SHA256" "-" "kernel.org tarball $FW_URL; only the blobs of the groups below are in the image, xz-compressed, byte-identical to this tag before compression" )
    rel SPDXRef-DOCUMENT DESCRIBES SPDXRef-linux-firmware
    while IFS="$(printf '\t')" read -r grp lic licsha cnt bytes; do
      gsum=$(awk -v g="$grp" '$3 == g {print $1 "  " $2}' "$FWDIR/firmware.list" | sha256sum | cut -d' ' -f1)
      lid="LicenseRef-linux-firmware-$grp"
      pkg "SPDXRef-firmware-$grp" "linux-firmware-$grp" "$(. "$OS_DIR/image/firmware/LINUX_FIRMWARE"; echo "$FW_TAG")" "$gsum" "-" "$cnt blobs, $bytes bytes xz-compressed; checksum is sha256 of the sorted 'sha256  path' list; licence file $lic (sha256 $licsha)"
      # licence on the package: declared and concluded are the vendor's redistributable terms
      jq -c --arg id "SPDXRef-firmware-$grp" --arg l "$lid" \
        'if .SPDXID == $id then .licenseDeclared = $l | .licenseConcluded = $l else . end' "$TMP/pkgs" > "$TMP/pkgs.new" && mv "$TMP/pkgs.new" "$TMP/pkgs"
      jq -nc --arg id "$lid" --arg name "$lic" --rawfile text "$FWDIR/licenses/$grp.$lic" \
        '{licenseId:$id, name:$name, extractedText:$text}' >> "$TMP/lics"
      rel SPDXRef-linux-firmware CONTAINS "SPDXRef-firmware-$grp"
    done < "$FWDIR/groups.tsv"
  fi

  . "$OS_DIR/model/LLAMA"
  LSUM="-"; [ -z "$LLAMA" ] || [ ! -f "$LLAMA" ] || LSUM=$(sha "$LLAMA")
  pkg SPDXRef-llama-cpp llama.cpp "$LLAMA_TAG" "$LSUM" "pkg:github/ggml-org/llama.cpp@$LLAMA_TAG" "commit $LLAMA_COMMIT; static llama-server build (checksum is the built binary)"
  rel SPDXRef-DOCUMENT DESCRIBES SPDXRef-llama-cpp

  if [ -n "$MODEL" ]; then
    . "$OS_DIR/model/MODEL"
    pkg SPDXRef-model-gguf "$(basename "$MODEL_URL")" "-" "$(sha "$MODEL")" "-" "GGUF model, url $MODEL_URL"
    rel SPDXRef-DOCUMENT DESCRIBES SPDXRef-model-gguf
  fi

  # Rust binaries and the crates each one links, resolved by cargo (Cargo.lock) for the musl target.
  for m in init authority-stub shell update; do
    pname=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$OS_DIR/$m/Cargo.toml" | head -1)
    bname=$(awk '/^\[\[bin\]\]/{b=1;next} b&&/^name = /{gsub(/"/,"",$3);print $3;exit}' "$OS_DIR/$m/Cargo.toml")
    [ -f "$BIN_DIR/$bname" ] || die "sbom: no binary $BIN_DIR/$bname"
    BID="SPDXRef-bin-$bname"
    pkg "$BID" "$bname" "0.1.0" "$(sha "$BIN_DIR/$bname")" "-" "Rust binary from crate $pname, git $COMMIT"
    rel SPDXRef-DOCUMENT DESCRIBES "$BID"
    cargo tree --manifest-path "$OS_DIR/Cargo.toml" -p "$pname" --target x86_64-unknown-linux-musl \
      -e normal --prefix none --locked 2>/dev/null \
      | sed -n 's/^\([A-Za-z0-9_.-][A-Za-z0-9_.-]*\) v\([0-9][^ ]*\).*/\1 \2/p' | sort -u > "$TMP/crates-$bname" \
      || true
    [ -s "$TMP/crates-$bname" ] || cargo tree --manifest-path "$OS_DIR/Cargo.toml" -p "$pname" -e normal --prefix none 2>/dev/null \
      | sed -n 's/^\([A-Za-z0-9_.-][A-Za-z0-9_.-]*\) v\([0-9][^ ]*\).*/\1 \2/p' | sort -u > "$TMP/crates-$bname"
    [ -s "$TMP/crates-$bname" ] || die "sbom: cargo tree gave no crates for $pname"
    while read -r cn cv; do
      [ "$cn" != "$pname" ] || continue
      CSUM=$(awk -v n="$cn" -v v="$cv" '
        /^\[\[package\]\]/{nm="";vr="";ck=""} /^name = /{gsub(/"/,"",$3);nm=$3} /^version = /{gsub(/"/,"",$3);vr=$3}
        /^checksum = /{gsub(/"/,"",$3);ck=$3; if(nm==n&&vr==v){print ck;exit}}' "$OS_DIR/Cargo.lock" 2>/dev/null || true)
      CID="SPDXRef-crate-$(printf '%s-%s' "$cn" "$cv" | tr -c 'A-Za-z0-9.-' '-')"
      grep -q "\"SPDXID\":\"$CID\"" "$TMP/pkgs" || pkg "$CID" "$cn" "$cv" "${CSUM:--}" "pkg:cargo/$cn@$cv" "crates.io checksum from Cargo.lock"
      rel "$BID" DEPENDS_ON "$CID"
    done < "$TMP/crates-$bname"
  done

  jq -n --slurpfile pk "$TMP/pkgs" --slurpfile rl "$TMP/rels" --slurpfile li "$TMP/lics" --arg created "$CREATED" --arg commit "$COMMIT" '
    {spdxVersion:"SPDX-2.3", dataLicense:"CC0-1.0", SPDXID:"SPDXRef-DOCUMENT",
     name:("allternit-os-" + $commit), documentNamespace:("https://allternit.com/spdx/os/" + $commit),
     creationInfo:{created:$created, creators:["Tool: allternit-os-provenance"]},
     packages:$pk, relationships:$rl}
    + (if ($li | length) > 0 then {hasExtractedLicensingInfos:$li} else {} end)' | jq -S . > "$OUTF"
  say "SBOM $OUTF: $(jq '.packages|length' "$OUTF") packages, sha256 $(sha "$OUTF")"
  ;;

manifest)
  OUTD=${1:?manifest OUTDIR}; KEY=${2:?key}; VER=${3:?version}; UKI=${4:?uki}; SBOM=${5:?sbom}
  [ $# -ge 11 ] || die "manifest needs both slots: VERITY SHA BYTES x2"
  shift 5
  [ -f "$KEY" ] || die "no signing key at $KEY"
  mkdir -p "$OUTD"
  USHA=$(sha "$UKI"); SSHA=$(sha "$SBOM")
  for n in a b; do
    VERITY=$1; ISHA=$2; IBYTES=$3; shift 3
    root=${VERITY%%:*}; rest=${VERITY#*:}; salt=${rest%%:*}; blocks=${rest#*:}
    jq -nSc --arg v "$VER" --arg slot "$n" --arg isha "$ISHA" --argjson ib "$IBYTES" --arg root "$root" \
      --arg salt "$salt" --argjson blocks "$blocks" --arg usha "$USHA" --arg ssha "$SSHA" \
      '{schema:1, version:$v, slot:$slot, image_sha256:$isha, image_bytes:$ib, verity_root:$root,
        verity_salt:$salt, data_blocks:$blocks, uki_sha256:$usha, sbom_sha256:$ssha}' \
      | tr -d '\n' > "$OUTD/manifest-$n.json"
    openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$OUTD/manifest-$n.json" -out "$OUTD/manifest-$n.sig"
    [ "$(wc -c < "$OUTD/manifest-$n.sig")" -eq 64 ] || die "signature is not 64 bytes"
  done
  say "signed manifests in $OUTD (version $VER, uki $USHA, sbom $SSHA)"
  ;;

verify)
  PUB=${1:?PUB.pem}; MAN=${2:?manifest}; SIG=${3:?sig}
  openssl pkeyutl -verify -pubin -inkey "$PUB" -rawin -in "$MAN" -sigfile "$SIG" >/dev/null \
    || die "BAD signature on $MAN"
  say "signature ok: $MAN"
  ;;

retarget)
  SRC=${1:?SRC.json}; KEY=${2:?key}; PFX=${3:?outprefix}; ISHA=${4:?sha}; IBYTES=${5:?bytes}
  jq -Sc --arg s "$ISHA" --argjson b "$IBYTES" '.image_sha256=$s | .image_bytes=$b' "$SRC" | tr -d '\n' > "$PFX.json"
  openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$PFX.json" -out "$PFX.sig"
  say "re-signed $PFX.json for sha256 $ISHA"
  ;;

enroll)
  VIN=${1:?VARS_IN}; VOUT=${2:?VARS_OUT}; KD=${3:?KEYDIR}
  command -v virt-fw-vars >/dev/null 2>&1 || die "virt-fw-vars not found (pip install virt-firmware)"
  G=$(cat "$KD/owner.guid")
  virt-fw-vars --input "$VIN" --output "$VOUT" \
    --set-pk "$G" "$KD/public/PK.crt" --add-kek "$G" "$KD/public/KEK.crt" --add-db "$G" "$KD/public/db.crt" \
    --secure-boot
  say "enrolled TEST PK/KEK/db into $VOUT (Secure Boot on)"
  ;;

*) sed -n '2,30p' "$0"; exit 2 ;;
esac
