import base64
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from vpn_config import check, ConfigError


KEY = base64.b64encode(bytes(range(32))).decode()


class VPNConfigTests(unittest.TestCase):
    def setUp(self):
        self.tmp = TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.env = self.root / ".env"
        self.secrets = self.root / "vpn"
        self.secrets.mkdir()

    def secret(self, name, value):
        path = self.secrets / name
        path.write_text(value)
        path.chmod(0o600)
        return path

    def test_supported_provider_pairs(self):
        cases = (
            ("protonvpn", "wireguard",
             ["wireguard_private_key"]),
            ("mullvad", "wireguard",
             ["wireguard_private_key", "wireguard_addresses"]),
            ("airvpn", "wireguard",
             ["wireguard_private_key", "wireguard_addresses",
              "wireguard_preshared_key"]),
            ("nordvpn", "wireguard",
             ["wireguard_private_key"]),
            ("nordvpn", "openvpn",
             ["openvpn_user", "openvpn_password"]),
            ("private internet access", "openvpn",
             ["openvpn_user", "openvpn_password"]),
        )

        for provider, protocol, names in cases:
            with self.subTest(provider=provider):
                self.env.write_text(
                    f'NORYLUM_VPN_PROVIDER="{provider}"\n'
                    f"NORYLUM_VPN_TYPE={protocol}\n"
                )

                for name in names:
                    value = (
                        "10.0.0.2/32"
                        if name == "wireguard_addresses"
                        else KEY if "key" in name else "example"
                    )
                    self.secret(name, value)

                self.assertEqual(
                    check(self.env, self.secrets),
                    (provider, protocol),
                )

    def test_unknown_provider_rejected(self):
        self.env.write_text(
            "NORYLUM_VPN_PROVIDER=unknown\n"
        )
        with self.assertRaises(ConfigError):
            check(self.env, self.secrets)

    def test_missing_secret_rejected(self):
        self.env.write_text(
            "NORYLUM_VPN_PROVIDER=protonvpn\n"
        )
        with self.assertRaises(ConfigError):
            check(self.env, self.secrets)

    def test_too_open_secret_rejected(self):
        self.env.write_text(
            "NORYLUM_VPN_PROVIDER=protonvpn\n"
        )
        path = self.secret("wireguard_private_key", KEY)
        path.chmod(0o644)

        with self.assertRaises(ConfigError):
            check(self.env, self.secrets)

    def test_symlink_rejected(self):
        self.env.write_text(
            "NORYLUM_VPN_PROVIDER=protonvpn\n"
        )
        target = self.root / "secret"
        target.write_text(KEY)
        target.chmod(0o600)

        (self.secrets / "wireguard_private_key").symlink_to(
            target
        )

        with self.assertRaises(ConfigError):
            check(self.env, self.secrets)

    def test_invalid_protocol_rejected(self):
        self.env.write_text(
            "NORYLUM_VPN_PROVIDER=mullvad\n"
            "NORYLUM_VPN_TYPE=openvpn\n"
        )
        with self.assertRaises(ConfigError):
            check(self.env, self.secrets)


if __name__ == "__main__":
    unittest.main()
