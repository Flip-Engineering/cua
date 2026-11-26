import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_flip_actions as chk  # noqa: E402

REPO = Path(__file__).resolve().parents[3]


def make_repo(body: str, listed: str = ".github/workflows/release.yml\n") -> Path:
    root = Path(tempfile.mkdtemp())
    (root / ".github" / "workflows").mkdir(parents=True)
    (root / ".github" / "workflows" / "release.yml").write_text(textwrap.dedent(body))
    (root / ".github" / "flip-active-workflows.txt").write_text(listed)
    return root


class StaticTest(unittest.TestCase):
    def test_the_forks_release_workflow_passes(self):
        self.assertEqual(chk.static(REPO), [])

    def test_self_hosted_lists_and_a_group_block_pass(self):
        root = make_repo("""
            on: workflow_dispatch
            jobs:
              a:
                runs-on: [self-hosted, Linux, X64, docker-linux]
                steps: []
              b:
                runs-on:
                  group: homelab
                  labels: [self-hosted, Linux, X64]
                steps: []
        """)
        self.assertEqual(chk.static(root), [])

    def test_a_github_machine_fails(self):
        root = make_repo("""
            jobs:
              a:
                runs-on: ubuntu-latest
                steps: []
              b:
                runs-on: [self-hosted, Linux]
        """)
        self.assertEqual(len(chk.static(root)), 1)
        self.assertIn("GitHub's machine ubuntu-latest", chk.static(root)[0])

    def test_a_label_list_without_self_hosted_fails(self):
        root = make_repo("""
            jobs:
              a:
                runs-on: [Linux, X64]
        """)
        self.assertIn("does not name self-hosted", chk.static(root)[0])

    def test_an_expression_fails(self):
        root = make_repo("""
            jobs:
              a:
                runs-on: ${{ matrix.os }}
        """)
        self.assertIn("computes runs-on", chk.static(root)[0])

    def test_a_job_without_runs_on_fails(self):
        root = make_repo("""
            jobs:
              a:
                uses: ./.github/workflows/other.yml
        """)
        self.assertIn("has no runs-on", chk.static(root)[0])

    def test_a_listed_file_that_is_missing_fails(self):
        root = make_repo("jobs:\n  a:\n    runs-on: [self-hosted]\n", ".github/workflows/gone.yml\n")
        self.assertIn("missing", chk.static(root)[0])


class LiveTest(unittest.TestCase):
    def test_only_listed_workflows_may_be_active(self):
        root = make_repo("jobs:\n  a:\n    runs-on: [self-hosted]\n")
        import os

        cwd = os.getcwd()
        os.chdir(root)
        try:
            self.assertEqual(chk.live(Path("."), [".github/workflows/release.yml"]), [])
            problems = chk.live(Path("."), [".github/workflows/release.yml", ".github/workflows/ci.yml"])
        finally:
            os.chdir(cwd)
        self.assertEqual(len(problems), 1)
        self.assertIn(".github/workflows/ci.yml is active", problems[0])


if __name__ == "__main__":
    unittest.main()
