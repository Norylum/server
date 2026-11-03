#!/usr/bin/env python3
"""Validation locale Gluetun : aucun reseau, aucun secret affiche."""

import base64
import binascii
import ipaddress
from pathlib import Path
import stat
import sys


class ConfigError(Exception):
    pass


REQUIREMENTS = {
    "protonvpn": {
        "wireguard": ("wireguard_private_key",),
        "openvpn": ("openvpn_user", "openvpn_password"),
    },
    "mullvad": {
        "wireguard": (
            "wireguard_private_key",
            "wireguard_addresses",
        ),
    },
    "airvpn": {
        "wireguard": (
            "wireguard_private_key",
            "wireguard_addresses",
            "wireguard_preshared_key",
        ),
    },
    "nordvpn": {
        "wireguard": ("wireguard_private_key",),
        "openvpn": ("openvpn_user", "openvpn_password"),
    },
    "private internet access": {
        "openvpn": ("openvpn_user", "openvpn_password"),
    },
}


def settings(path):
    if not path.is_file() or path.is_symlink():
        raise ConfigError(".env absent ou non fiable")

    result = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()

        if not line or line.startswith("#") or "=" not in line:
            continue

        key, value = line.split("=", 1)
        key = key.strip()

        if key not in (
            "NORYLUM_VPN_PROVIDER",
            "NORYLUM_VPN_TYPE",
        ):
            continue

        value = value.strip()
        if (
            len(value) >= 2
            and value[0] == value[-1]
            and value[0] in ("'", '"')
        ):
            value = value[1:-1]

        result[key] = value

    return result


def check(
    env_path=Path(".env"),
    secret_dir=Path("vpn-secrets"),
):
    env = settings(env_path)
    provider = env.get("NORYLUM_VPN_PROVIDER", "")
    protocol = env.get("NORYLUM_VPN_TYPE", "wireguard")

    if provider not in REQUIREMENTS:
        raise ConfigError(
            "Fournisseur VPN non configure ou non supporte"
        )

    supported = REQUIREMENTS[provider]
    if protocol not in supported:
        raise ConfigError(
            f"Protocole {protocol} non pris en charge "
            "pour ce fournisseur"
        )

    if secret_dir.is_symlink() or not secret_dir.is_dir():
        raise ConfigError(
            "vpn-secrets absent ou lien symbolique refuse"
        )

    secrets = {}

    for name in supported[protocol]:
        path = secret_dir / name

        try:
            info = path.lstat()
        except FileNotFoundError:
            raise ConfigError(
                f"Secret manquant : {name}"
            ) from None

        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_size < 1
            or info.st_size > 4096
        ):
            raise ConfigError(
                f"Fichier de secret invalide : {name}"
            )

        if info.st_mode & 0o077:
            raise ConfigError(
                f"Permissions trop ouvertes (chmod 600) : {name}"
            )

        value = path.read_text(encoding="utf-8").strip()

        if not value:
            raise ConfigError(f"Secret vide : {name}")

        secrets[name] = value

    for name in (
        "wireguard_private_key",
        "wireguard_preshared_key",
    ):
        if name in secrets:
            try:
                decoded = base64.b64decode(
                    secrets[name], validate=True
                )
            except (binascii.Error, ValueError):
                raise ConfigError(
                    f"Format WireGuard incorrect : {name}"
                ) from None

            if len(decoded) != 32:
                raise ConfigError(
                    f"Longueur WireGuard incorrecte : {name}"
                )

    if "wireguard_addresses" in secrets:
        try:
            address = ipaddress.ip_interface(
                secrets["wireguard_addresses"]
                .split(",", 1)[0]
                .strip()
            )
        except ValueError:
            raise ConfigError(
                "Adresse WireGuard invalide"
            ) from None

        if address.version != 4:
            raise ConfigError(
                "Adresse WireGuard IPv4 requise"
            )

    return provider, protocol


if __name__ == "__main__":
    try:
        provider, protocol = check()
    except (ConfigError, UnicodeError, OSError) as error:
        print(f"VPN bloque : {error}", file=sys.stderr)
        sys.exit(1)

    print(
        f"OK : configuration locale {provider} / {protocol} "
        "(aucun secret affiche)"
    )
