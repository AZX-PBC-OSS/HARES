# Submodules

The repository vendors its two reference implementations as git
submodules under `vendors/`:

| submodule | role | url |
|---|---|---|
| `vendors/OCHRE` | the Python reference implementation the equipment-level and whole-building parity tests run against | https://github.com/AZX-PBC-OSS/OCHRE.git |
| `vendors/EnergyPlus` | the reference engine whose warm-up convergence, weather interpolation and unavailable-period rules HARES follows and cites | https://github.com/NatLabRockies/EnergyPlus.git |

Both URLs are https: a fresh clone on a machine with no SSH keys runs
`git clone --recursive` (or `git clone` followed by
`git submodule update --init --recursive`) and needs nothing else.

## The pins

Each submodule is pinned to one recorded commit; `git ls-tree HEAD
vendors/` prints both pins, and a checkout of any commit of this
repository lands on exactly the commits that commit recorded. The pin
is the only version selector: neither entry carries a `branch` hint in
`.gitmodules` (the OCHRE entry's hint was removed), and `git submodule
update` always checks out the recorded commit. Never use `git submodule
update --remote`: it ignores the pin and moves the checkout to a
branch head, which is not a version this repository has verified.

The two pins are recorded in different places, on purpose:

- The OCHRE pin's single home is the "Vendored commit" line in
  [docs/validation.md](validation.md), and
  `scripts/check-ochre-commit.sh` fails when that line and the checked
  out submodule disagree. Update that line in the same commit as the
  gitlink.
- The EnergyPlus pin has no other record; the gitlink that `git
  ls-tree HEAD vendors/EnergyPlus` prints is the source of truth for
  it.

## Updating a pin

1. Read the current pin and pick the target commit:
   `git ls-tree HEAD vendors/<name>`.
2. Move the pin and commit the gitlink:

   ```bash
   git -C vendors/OCHRE fetch origin
   git -C vendors/OCHRE checkout <commit>
   git add vendors/OCHRE
   ```

   Never rewrite `.gitmodules` to move a pin: the URLs stay as they
   are.
3. Update the pin's record in the same commit (for OCHRE, the
   "Vendored commit" line in docs/validation.md; the check script
   fails the commit otherwise).
4. Re-run what the pin feeds before the commit lands:
   - OCHRE: the parity suites the CI `ochre-python-test` job runs
     (`uv run pytest -m ochre --fail-on-skip`). The vendored
     reference's numbers are the parity comparison's baseline, so a
     moved pin re-runs them, and where the reference itself moved, the
     parity records re-capture with the cause. The golden fixtures
     re-capture only if the engine's own numbers move.
   - EnergyPlus: the citation checks and the comparisons the vendored
     source feeds (`bash scripts/check-energyplus-sections.sh`, and
     the parity suites above where the engine's reference behaviour
     enters them).
5. A pin move is its own commit, so a bisect lands inside one
   reference version.

Existing clones keep the URL they were initialized with: after a URL
change in `.gitmodules`, run `git submodule sync <path>` once before
the next `git submodule update`.
