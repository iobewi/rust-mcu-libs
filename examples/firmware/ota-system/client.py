#!/usr/bin/env python3
"""Upload an ESP image with bounded memory, then optionally activate that ID."""
import argparse
import hashlib
import http.client
import os
from pathlib import Path
import ssl
from urllib.parse import urlsplit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("url", help="https://device address, without path")
    parser.add_argument("image", type=Path)
    parser.add_argument("deployment_id")
    parser.add_argument("--ca", required=True, help="trusted server certificate/CA PEM")
    parser.add_argument("--domain", choices=("system", "component"), default="system")
    parser.add_argument("--version", default="1")
    parser.add_argument("--runtime-api", default="1.0")
    parser.add_argument("--activate", action="store_true")
    args = parser.parse_args()
    url = urlsplit(args.url)
    if url.scheme != "https" or not url.hostname or url.path not in ("", "/") or url.username or url.query or url.fragment:
        parser.error("provide an HTTPS origin")
    token = os.environ.get("IOBEWI_OTA_TOKEN", "")
    if not token or any(c in token for c in "\r\n"):
        parser.error("set IOBEWI_OTA_TOKEN")
    if not 1 <= len(args.deployment_id) <= 64 or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_." for c in args.deployment_id):
        parser.error("invalid deployment id")
    endpoint = "/ota" if args.domain == "system" else "/workload"
    context = ssl.create_default_context(cafile=args.ca)
    with args.image.open("rb") as image:
        digest = hashlib.file_digest(image, "sha256").hexdigest()
        # file_digest may bypass the file object's position; obtain size directly.
        length = os.fstat(image.fileno()).st_size
        if not length:
            parser.error("empty image")
        image.seek(0)
        connection = http.client.HTTPSConnection(url.hostname, url.port or 443, context=context, timeout=60)
        connection.putrequest("POST", endpoint + "/upload")
        for name, value in {
            "Authorization": "Bearer " + token,
            "X-Deployment-ID": args.deployment_id,
            "X-Image-SHA256": "sha256:" + digest,
            "Content-Length": str(length),
        }.items():
            connection.putheader(name, value)
        if args.domain == "component":
            connection.putheader("X-Component-Version", args.version)
            connection.putheader("X-Runtime-Api", args.runtime_api)
        connection.endheaders()
        remaining = length
        while remaining:
            chunk = image.read(min(2048, remaining))
            if not chunk:
                raise RuntimeError("image changed during upload")
            connection.send(chunk)
            remaining -= len(chunk)
        response = connection.getresponse()
        result = response.read(2048).decode()
        connection.close()
        if response.status != 200 or result != "staged":
            raise RuntimeError(f"upload failed: {response.status} {result}")
        print("staged:", args.deployment_id)
    if args.activate:
        connection = http.client.HTTPSConnection(url.hostname, url.port or 443, context=context, timeout=60)
        connection.request("POST", endpoint + "/activate", body=b"", headers={
            "Authorization": "Bearer " + token, "X-Deployment-ID": args.deployment_id,
        })
        response = connection.getresponse()
        result = response.read(2048).decode()
        connection.close()
        if response.status != 200 or result != ("activating" if args.domain == "system" else "pending"):
            raise RuntimeError(f"activation failed: {response.status} {result}")
        print("activating; inspect device logs for confirmation or rollback")


if __name__ == "__main__":
    main()
