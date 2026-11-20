#!/usr/bin/env bash
# Sign a macOS release binary with the Developer ID certificate and have
# Apple notarize it.
#
#   scripts/sign-macos.sh path/to/quome
#
# Why: on macOS the agent key is a Keychain item that only the program that
# created it may read without asking. An unsigned (ad-hoc) binary is a new
# program to the Keychain on every release, so every upgrade asked again.
# A Developer ID signature with a fixed identifier makes every release the
# same program.
#
# Needs, in the environment (GitHub Actions secrets in the workflows):
#   MACOS_CERT_P12_BASE64    the "Developer ID Application" identity, .p12
#   MACOS_CERT_PASSWORD      the .p12's password
#   APPLE_API_KEY_P8_BASE64  an App Store Connect API key, for notarization
#   APPLE_API_KEY_ID         its key ID
#   APPLE_API_ISSUER_ID      its issuer ID
#
# The certificate goes into a throwaway keychain that is deleted on exit;
# nothing is written outside a temp directory.
set -euo pipefail

bin="${1:?usage: sign-macos.sh BINARY}"
[ -f "$bin" ] || { echo "no such file: $bin" >&2; exit 2; }
for v in MACOS_CERT_P12_BASE64 MACOS_CERT_PASSWORD APPLE_API_KEY_P8_BASE64 APPLE_API_KEY_ID APPLE_API_ISSUER_ID; do
  [ -n "${!v:-}" ] || { echo "$v is not set" >&2; exit 2; }
done

# The identifier is part of what the Keychain remembers. Changing it makes
# every user's next upgrade ask again.
IDENTIFIER="cloud.quome.cli"
# Apple's "Developer ID - G2" intermediate, which the leaf chains to. The
# .p12 does not carry it and a runner image is not guaranteed to.
INTERMEDIATE_URL="https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer"
INTERMEDIATE_SHA256="f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a"

tmp="$(mktemp -d)"
kc="$tmp/signing.keychain-db"
kc_pw="$(openssl rand -hex 24)"
saved_list="$(security list-keychains -d user | sed 's/^ *"//; s/"$//')"
cleanup() {
  # shellcheck disable=SC2086
  security list-keychains -d user -s $saved_list >/dev/null 2>&1 || true
  security delete-keychain "$kc" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

printf '%s' "$MACOS_CERT_P12_BASE64" | base64 --decode >"$tmp/cert.p12"
printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode >"$tmp/AuthKey.p8"
curl -fsSL "$INTERMEDIATE_URL" -o "$tmp/intermediate.cer"
echo "$INTERMEDIATE_SHA256  $tmp/intermediate.cer" | shasum -a 256 -c - >/dev/null

security create-keychain -p "$kc_pw" "$kc"
security set-keychain-settings -lut 3600 "$kc"
security unlock-keychain -p "$kc_pw" "$kc"
security import "$tmp/intermediate.cer" -k "$kc" >/dev/null
security import "$tmp/cert.p12" -k "$kc" -P "$MACOS_CERT_PASSWORD" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple: -s -k "$kc_pw" "$kc" >/dev/null
# shellcheck disable=SC2086
security list-keychains -d user -s "$kc" $saved_list

identity="$(security find-identity -v -p codesigning "$kc" | awk -F'"' '/Developer ID Application/ {print $2; exit}')"
[ -n "$identity" ] || { echo "no valid Developer ID Application identity in the .p12" >&2; exit 1; }
echo "signing as: $identity"

codesign --force --timestamp --options runtime \
  --identifier "$IDENTIFIER" --sign "$identity" --keychain "$kc" "$bin"

# What the Keychain will hold every later release to.
codesign --verify --strict --verbose=2 "$bin"
info="$(codesign -dvv "$bin" 2>&1)"
echo "$info" | grep -E '^(Identifier|TeamIdentifier|Authority|Timestamp|flags|CodeDirectory)' || true
echo "$info" | grep -qx "Identifier=$IDENTIFIER" || { echo "identifier is not $IDENTIFIER" >&2; exit 1; }
echo "$info" | grep -q '^Authority=Developer ID Application: ' || { echo "not Developer ID signed" >&2; exit 1; }
echo "$info" | grep -q 'flags=.*runtime' || { echo "hardened runtime is off" >&2; exit 1; }
if echo "$info" | grep -q 'Signature=adhoc'; then echo "still ad-hoc signed" >&2; exit 1; fi

# Notarization. A bare executable cannot be stapled; Gatekeeper looks the
# ticket up online, and Homebrew installs do not quarantine the file anyway.
ditto -c -k --keepParent "$bin" "$tmp/quome.zip"
xcrun notarytool submit "$tmp/quome.zip" \
  --key "$tmp/AuthKey.p8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" \
  --wait --timeout 30m --output-format json >"$tmp/notary.json" || true
status="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status",""))' "$tmp/notary.json" 2>/dev/null || true)"
id="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id",""))' "$tmp/notary.json" 2>/dev/null || true)"
echo "notarization: ${status:-no result} ${id}"
if [ "$status" != "Accepted" ]; then
  [ -z "$id" ] || xcrun notarytool log "$id" \
    --key "$tmp/AuthKey.p8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" || true
  exit 1
fi
