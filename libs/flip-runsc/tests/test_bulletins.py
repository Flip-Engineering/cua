import io
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import bulletins as bl  # noqa: E402

TABLE = """| **CVE Addressed** | **Affected Products** | **Platform or OS** | **Affected Versions** | **Updated Version** |
| --- | --- | --- | --- | --- |
| CVE-2026-47604 | GeForce | Linux(R580) | All driver versions prior to 580.178.04 | 580.178.04 |
| CVE-2026-47604 | Tesla | Linux(R580) | All driver versions prior to 580.178.04 | 580.178.04 |
| CVE-2026-47604 | GeForce | Linux(R595) | All driver versions prior to 595.91.07 | 595.91.07 |
| CVE-2026-47603 | Tesla | Linux(R580) | All driver versions prior to 580.178.04 | 580.178.04 |
| CVE-2026-47604 | GeForce | Windows(R580) | All driver versions prior to 582.78 | 582.78 |
"""


def test_parse_keeps_linux_branches_with_their_fix_and_cves():
    out = bl.parse(TABLE)
    assert out == {
        "580": {"fixed_in": "580.178.04", "cves": ["CVE-2026-47604", "CVE-2026-47603"]},
        "595": {"fixed_in": "595.91.07", "cves": ["CVE-2026-47604"]},
    }


def test_refresh_keeps_driver_bulletins_and_skips_others():
    files = {
        "https://api.github.com/repos/NVIDIA/product-security/contents/": [
            {"name": "2026", "type": "dir"},
            {"name": "README.md", "type": "file"},
        ],
        "https://api.github.com/repos/NVIDIA/product-security/contents/2026": [
            {"name": "5861", "type": "dir"},
            {"name": "5862", "type": "dir"},
        ],
        "2026/5861/5861.md": TABLE,
        "2026/5861/5861.json": {
            "document": {
                "title": "Security Bulletin: NVIDIA GPU Display Driver - November 2026",
                "tracking": {"current_release_date": "2026-11-20"},
            }
        },
        "2026/5862/5862.md": "| CVE-2026-1 | DGX | Linux | x | 1.2 |\n",
    }

    def opener(req, timeout):
        url = req.full_url
        for key, body in files.items():
            if url.endswith(key) or url.rstrip("/") == key.rstrip("/"):
                return io.BytesIO((body if isinstance(body, str) else json.dumps(body)).encode())
        raise AssertionError(url)

    rows = bl.refresh(None, opener)
    assert [r["bulletin"] for r in rows] == ["5861"]
    assert rows[0]["linux"]["580"]["fixed_in"] == "580.178.04"
    assert rows[0]["date"] == "2026-11-20"


def test_text_form_has_one_line_per_branch_for_the_updater():
    rows = [
        {
            "bulletin": "5861",
            "date": "2026-11-20",
            "url": "u",
            "linux": {"580": {"fixed_in": "580.178.04", "cves": ["a", "b"]}},
        }
    ]
    assert bl.text(rows) == "580 580.178.04 5861 2 u\n"
