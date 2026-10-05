"""Typed surface of the optional ``boto3`` package used by HARES.

The published ``boto3-stubs`` package declares per-service ``client()``
overloads that return unknown for every service whose stub package is not
installed, so the member type is partially unknown under strict mode. This
stub models the anonymous-download surface HARES uses.
"""

from __future__ import annotations

from botocore.config import Config


class S3Client:
    def download_file(self, bucket: str, key: str, filename: str) -> None: ...


def client(
    service_name: str,
    region_name: str | None = ...,
    api_version: str | None = ...,
    use_ssl: bool | None = ...,
    verify: bool | str | None = ...,
    endpoint_url: str | None = ...,
    aws_access_key_id: str | None = ...,
    aws_secret_access_key: str | None = ...,
    aws_session_token: str | None = ...,
    config: Config | None = ...,
) -> S3Client: ...
