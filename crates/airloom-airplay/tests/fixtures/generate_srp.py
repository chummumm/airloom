"""Regenerate the non-secret interoperability fixture with srptools==1.0.1.

Run manually with that MIT-licensed Python package installed. Production Rust
does not depend on Python. Fixed test keys deliberately provide reproducibility.
"""

import hashlib
import json
from pathlib import Path

from srptools import SRPClientSession, SRPContext, SRPServerSession, constants

password = "airloom-fixture-not-a-secret"
salt = bytes(range(16)).hex()
client_private = bytes(range(32, 64)).hex()
server_private = bytes(range(64, 96)).hex()
context = SRPContext(
    "Pair-Setup",
    password,
    prime=constants.PRIME_3072,
    generator=constants.PRIME_3072_GEN,
    hash_func=hashlib.sha512,
)
verifier = context.get_common_password_verifier(
    context.get_common_password_hash(bytes.fromhex(salt))
)
server = SRPServerSession(context, format(verifier, "x"), server_private)
client = SRPClientSession(context, client_private)
client.process(server.public, salt)
server.process(client.public, salt)
assert server.verify_proof(client.key_proof)
assert client.verify_proof(server.key_proof_hash)
assert client.key == server.key

def string(value):
    return value.decode() if isinstance(value, bytes) else value

fixture = {
    "generator": "srptools 1.0.1; SHA512; RFC5054 3072-bit group; Pair-Setup identity",
    "password": password,
    "salt": salt,
    "client_private": client_private,
    "server_private": server_private,
    "verifier": format(verifier, "x"),
    "client_public": client.public,
    "server_public": server.public,
    "session_key": string(client.key),
    "client_proof": string(client.key_proof),
    "server_proof": string(server.key_proof_hash),
}
Path(__file__).with_name("srp_sha512_3072.json").write_text(
    json.dumps(fixture, indent=2) + "\n", encoding="utf-8"
)
