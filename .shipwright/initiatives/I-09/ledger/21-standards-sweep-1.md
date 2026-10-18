# The standards and papers sweep, record 1

External-standards research for I-09, 2026-10-17. Mandate: the charter
(00-charter.md), success criteria 1 and 6. Starting set: the fan-out plan's
Wave 5 (docs/superpowers/plans/2026-10-17-hares-hardening-fanout.md, Tasks
5.1-5.5). Every claim names the fetched source (URL plus what it says).
Paywalled standard text is marked UNVERIFIABLE-WEB with the exact scope the
operator should supply from library access. No em dashes are used in this
file.

HARES's current validation surface, for reference (verified in the tree,
not the web): `tests/bestest/` runs cases 600, 900, 600FF, 900FF and 640
with bands pinned in `tests/bestest/reference_bands.rs` from ASHRAE
140-2017 Tables B8-2 and B8-3a (docs/validation.md); fixtures exist for
610, 620, ce100, ce200 and a heat-pump case but the I-08 envelope review
(docs/superpowers/plans/2026-10-17-hares-hardening-reviews/2-envelope.md,
line 45) records that only 600, 900, 600FF, 900FF and 640 actually run,
with 620, 650, 920, 950, 195 and 395 missing. ISO 13786 is not implemented.
The vendored Ruby cites ANSI/RESNET/ICC 301 for equipment equations and
schedule defaults.

---

## 1. The validation landscape

### 1.1 A note on the mission's "AA/C0" shorthand

"AA/C0" does not match any designation verifiable in the public standard
text or in the HARES tree (grepped docs/ and docs/superpowers/). The
operative public structure of ASHRAE 140 is: Section 7 "Building Thermal
Envelope and Fabric Load Tests", organized as low-mass basic cases
(600-series), high-mass basic cases (900-series), free-float cases (FF),
the sunspace 960, plus in-depth cases and the acceptance-range procedure
in Annex B8 (140-2017 and 140-2020) or Annex B22 (140-2011). The most
plausible reading of "AA/C0" is "ANSI/ASHRAE (AA) comparative (C-series)
case 0 family". Flag for the operator: confirm the intended mapping before
the phrase enters a row spec. Everything below uses the standard's own
designations.

### 1.2 Which ASHRAE 140 cases apply to a residential-capable engine

Two residential-relevant suites live inside ASHRAE 140 Section 7:

(a) The single-zone BESTEST house (600/900 series). Source fetched:
docs.betterbuilding.io, "ANSI/ASHRAE Standard 140 - Building Thermal
Envelope and Fabric Load Tests" (https://docs.betterbuilding.io/support-and-training/other-support/testing-and-verification/ansi-ashrae-standard-140-building-thermal-envelope-and-fabric-load-tests),
a full public validation report against ANSI/ASHRAE 140-2020 Section 7.
It enumerates every case with its spec reference: 600 base, 610 south
overhang, 620 east/west windows, 630 east/west shading, 640 thermostat
setback, 650 night ventilation (1703.16 m3/h, heating disabled), 660
low-e windows, 670 single-pane, 680 increased insulation, 685 zero
deadband, 695 combined; the 900-series repeats with heavy walls and
floors (900, 910, 920, 930, 940, 950, 960 sunspace two-zone, 980, 985,
995); free-float cases 600FF, 650FF, 680FF, 900FF, 950FF, 980FF. It also
pins the required modelling configuration (TMY2 station 725650, ground
temperature 10 C, reflectance 0.20, TARP inside convection, DOE-2 outside
convection, solar absorptance 0.6, emittance 0.9, 200 W gains 60/40
radiative/convective, 0.5 ACH, setpoints 20/27 C) and reports a 28-of-28
pass for an EnergyPlus-based tool against the Annex B8 and B16 ranges.

(b) The HERS BESTEST house (the genuinely residential geometry). Source
fetched: NREL/NatLabRockies "EnergyPlusValidationReports", ASHRAE140-HERS
report (https://github.com/NatLabRockies/EnergyPlusValidationReports/blob/master/ASHRAE140-HERS/Report/ASHRAE140-HERS.md,
repo archived June 2026, read-only). It documents Section 7 of ASHRAE
140-2011 as inherited from HERS BESTEST (NREL/TP-472-7332a, Judkoff and
Neymark 1995): a single-story 27 by 57 ft house with conditioned first
floor, unconditioned attic and raised floor. Tier I cases: L100A base,
L110A high infiltration, L120A well insulated walls and roof, L130A
low-e double pane, L140A zero window area, L150A south windows, L155A
south overhang, L160A east/west windows, L165A east/west shading,
L170A no internal loads, L200A energy-inefficient, L202A low exterior
absorptance, L302A uninsulated slab, L304A insulated slab, L322A
uninsulated conditioned basement, L324A insulated basement. Tier II
passive solar cases: P100A, P105A, P110A, P140A, P150A. Weather:
Colorado Springs (heating) and Las Vegas (cooling). The report states
the acceptance-range procedure: Annex B22 of 140-2011 uses standard
deviations and 90 percent confidence limits with 4 million Btu offsets
on the reference program spread.

Where the published pass criteria live (public, fetched):

- ASHRAE 140-2017 errata, ashrae.org (https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20errata/standards/140_2017_a_20200901.pdf):
  downloaded and text-extracted. It confirms the Annex B8 results-table
  structure: B8-2 annual sensible cooling, B8-3 peak heating, B8-5
  free-float temperature output, B8-6/B8-7 basic sensitivity, B8-8/B8-9
  in-depth cases 195 through 470, B8-14 Case 600 annual transmitted
  solar unshaded. The numeric bands themselves live in the spreadsheet
  tables the standard renders; HARES's pinned bands (600 heating
  4,296 to 5,709 kWh/yr, cooling 6,137 to 7,964 kWh/yr, 600FF peak
  64.9 to 69.5 C, docs/validation.md) are consistent with the band
  values quoted across the public validation reports above, but the
  errata text itself does not restate them as prose.

- Where the numeric bands themselves live: the widely published values
  are the standard's Annex B8 spreadsheet tables; HARES's pinned bands
  (600 heating 4,296 to 5,709 kWh/yr, cooling 6,137 to 7,964 kWh/yr,
  600FF peak 64.9 to 69.5 C, docs/validation.md) cite Table B8-2 and
  B8-3a and are the in-tree contract. The public reports fetched above
  reproduce the case list, methodology and per-tool results (the Better
  Building report prints its own 140-2020 results per case in MWh and
  kW; the NREL HERS report describes the Annex B22 acceptance-range
  construction), but neither restates every band row in prose. Treat
  the per-case numeric band rows as standard-text content: see the
  UNVERIFIABLE-WEB note below.

- UNVERIFIABLE-WEB: the normative text of ASHRAE 140-2020 (and 140-2017)
  including the exact Annex B8/B16 minimum-maximum rows per case and
  the Section 7 case construction tables (Tables 5-1 through 5-13,
  Informative Table 5-26). The Better Building report and the EnergyPlus
  HERS report reproduce the case list and methodology publicly, but the
  operator should supply the standard's own table pages from library
  access before any row pins 140-2020 bands as the contract.

What each case proves, compressed: 600/900 prove annual load physics
against the reference spread; 610/630 prove shading; 620/920 prove
orientation-dependent solar; 640/940 prove setback and storage
interaction; 650/950 prove large-volume mechanical ventilation with
heating disabled; 660/670 prove glazing property sweeps; 680/695/980/985/
995 prove insulation and deadband interactions; 6xxFF/9xxFF prove peak
and minimum free-float temperatures; 960 proves two-zone coupling with an
unconditioned buffer; the HERS L-series proves a residential geometry
with an unconditioned attic, raised floor, slab and basement variants;
the P-series proves interior mass placement.

Can HARES run these today:

- 600, 900, 600FF, 900FF, 640: yes (running, banded).
- 610, 620: fixtures exist, not wired; bands trivial to add from the
  public 140-2020 report ranges once the operator confirms edition.
  Row-shaped gap: wire cases and pin bands in code, same shape as
  Task 5.2.
- 650, 950: need night ventilation at 1703.16 m3/h with heating fully
  disabled; HARES has mechanical ventilation equipment but the ideal
  "heating never fires" contract must be explicit in the fixture. Gap
  row.
- 660, 670, 680, 685, 695 and the 9xx counterparts: glazing sweeps are
  input-level; the zero-deadband thermostat (685/985) needs a deadband
  override HARES's thermostat supports. Gap row.
- 960 sunspace: requires two zones with an unconditioned buffer zone;
  HARES models multi-zone dwellings, so this is a fixture problem, not
  an engine gap, provided unconditioned-zone criteria exist (the I-08
  core-physics review asks for exactly that).
- HERS L100A to L170A, L200A to L202A: needs the residential attic
  geometry, raised floor and window-per-orientation inputs; HARES's
  HPXML lineage covers these input types. Feasible now; no row exists.
- L302A to L324A (slabs and basements): needs ground coupling of the
  slab/basement kind; HARES's ground coupling is the open Task 1.8 /
  195/395 territory. Hard dependency: land 5.2's floor-warmup cases
  first.
- P100A to P150A (interior mass walls): needs interior-partition mass
  input; feasible if HARES exposes partition walls in fixtures. Gap
  row, after the L-series.

### 1.3 ISO 13786 (dynamic thermal characteristics)

Source fetched: ISO's Own Book Preview pages for ISO 13786 ed-3
(https://www.iso.org/obp/ui/fr/#iso:std:iso:13786:ed-3:v2:en:ref:4 and
the ed-2 page https://www.iso.org/obp/ui#iso:std:iso:13786:ed-2:en) and
the iTeh catalog entry for EN ISO 13786:2017
(https://standards.iteh.ai/catalog/standards/cen/20dc4954-e122-47d9-9642-f73aa8211b21/en-iso-13786-2017).
What they say: the standard defines the calculation method for dynamic
thermal characteristics of building components (periodic thermal
transmittance, areal heat capacity, decrement factor, phase lag) for
harmonic (annual cycle) boundary conditions; Annex B gives the basic
principles and worked application examples; Annex C gives simpler
estimation methods for heat capacities in limited cases. A REHVA Journal
overview (https://www.rehva.eu/rehva-journal/chapter/epb-standards-on-hygrothermal-performance-of-building-components-and-building-elements)
confirms 13786's role as the EPB calculation method for component
dynamics.

ISO 13786 does not ship a public multi-case "verification set" with pass
bands the way ASHRAE 140 does; the verification practice is to reproduce
the Annex B/C worked values and to cross-check the periodic quantities
against independent published implementations. Public cross-check
sources: The Concrete Centre "Dynamic Thermal Properties Calculator"
user guide (https://www.concretecentre.com/TCC/media/TCCMediaLibrary/Products/Dynamic-thermal-properties-calculator-user-guide.pdf),
which implements ISO 13786 areal heat capacity and states its use as an
effective heat capacity measure; and the peer-reviewed periodic
transmittance formulation literature (for example the Energy and
Buildings paper evaluating dynamic envelope performance via periodic
thermal transmittance, time lag and decrement factor,
https://www.sciencedirect.com/science/article/am/pii/S0378778821006265).

Can HARES run it today: no. Task 5.2 adds areal capacity, decrement
factor and phase lag per construction. Row-shaped gap beyond 5.2: pin a
verification table (per construction: areal heat capacity, decrement
factor, lag) against the Annex B/C worked values plus one independent
implementation, with the standard text as the normative reference.

- UNVERIFIABLE-WEB: the full ISO 13786:2017 text including Annex B
  worked examples and Annex C formulas. The operator should supply the
  standard PDF; the public pages confirm scope and annex structure
  only.

### 1.4 ANSI/RESNET/ICC 301 and CIBSE

RESNET 301. Sources fetched: the OpenStudio-HPXML documentation
(https://openstudio-hpxml.readthedocs.io/en/v1.8.1/workflow_inputs.html),
which states: occupant-number defaults and end-use defaults follow
ANSI/RESNET/ICC 301-2019 and 301-2022; the default occupancy schedule
fractions are Table C.3(5) of ANSI/RESNET/ICC 301-2022 Addendum C
(24-hour weekday fractions quoted in full in the docs); the general
water-use schedule is also Table C.3(5); emissions defaults (CO2e,
NOx, SO2 per fuel in lb/MBtu) are from ANSI/RESNET/ICC 301-2022
Addendum B. HARES's vendored Ruby cites the same standard throughout
(hotwater_appliances.rb: "Source: ANSI/RESNET/ICC 301", clothes washer
IMEF and dryer CEF interpretations, HERS Addendum 81/82/90f equations).

What applies to HARES: 301 is not a physics validation suite; it is the
normative source of residential schedule fractions, internal-gain
defaults and reference-equation forms that HARES already inherits. The
check HARES can adopt is conformance of its defaults and generated
schedules to 301-2022 Addendum B and C values, which OS-HPXML publishes
openly.

Can HARES run it today: the defaults are already in the tree via the
vendored OS-HPXML Ruby; nothing tests them as a contract. Row-shaped
gap: a golden-test row pinning the 301-derived schedule fractions,
water-use defaults and emissions factors, citing the OS-HPXML docs page
as the public witness and marking the 301-2022 standard text
UNVERIFIABLE-WEB (supply from library access: Addendum B Table B-x
emissions factors, Addendum C Table C.3(5)).

CIBSE TM63. Source fetched: CIBSE's knowledge portal entry
(https://www.cibse.org/knowledge-research/knowledge-portal/operational-performance-building-performance-modelling-and-calibration-for-evaluation-of-energy-in-use-tm63/):
"TM63: Operational performance: Modelling for evaluation of energy
in-use aims to provide a methodological framework to undertake
measurement and verification"; and the Jain et al. 2021 paper (Building
Services Engineering Research and Technology,
https://journals.sagepub.com/doi/10.1177/01436244211008317, open copy at
https://discovery.ucl.ac.uk/id/eprint/10126699/1/01436244211008317.pdf):
TM63 provides the calibration framework for understanding the
performance gap and pinpointing operational inefficiencies. Relevance to
HARES: TM63 is the citeable methodology for HARES's calibration surface
(Task 4.5, row B7): the fit paths, the reported residual structure and
the performance-gap narrative. It is not an envelope test. Row-shaped
gap: a documentation row adopting TM63's vocabulary (calibrated
uncertainty, reporting basis) for the calibration system's docs and fit
reports.

- UNVERIFIABLE-WEB: CIBSE TM63 full text (paywalled on cibse.org).
  Scope to supply: the calibration reporting template and acceptance
  criteria sections.

### 1.5 IEA EBC Annex 58 and Annex 71 (empirical validation)

Annex 58. Sources fetched: the EBC Annex 58 Project Summary Report
(https://www.iea-ebc.org/Data/publications/EBC_PSR_Annex_58.pdf),
downloaded and text-extracted. What it says: Annex 58 (2011-2015,
"Reliable building energy performance characterisation based on full
scale dynamic measurements") built empirical validation datasets for
simulation programs; the empirical validation experiment was run at the
Twin Houses of Fraunhofer IBP, Holzkirchen, Germany; subtasks covered
in-situ dynamic testing and data analysis (Subtask 1), the empirical
validation exercise (Subtask 2, summarized at
https://eprints.leedsbeckett.ac.uk/3114/), and "Empirical validation of
common building energy simulation" (Subtask 4a; the ResearchGate record
describes the full-scale empirical validation dataset development for
BES tools). The dynastee network page
(https://dynastee.info/network/iea-ecbcs-annex-58/) hosts the subtask
structure.

Annex 71. Sources fetched: the Dynastee "EBC annex 71 reports" page
(https://dynastee.info/publications/ebc-annex-71-reports/) listing four
public subtask report PDFs, of which the fourth,
"EBC_Annex71-BES-validation-exercise.pdf"
(https://dynastee.info/wp-content/uploads/2021/09/EBC_Annex71-BES-validation-exercise.pdf),
was downloaded and text-extracted. What it says: the Annex 71
BES-validation exercise investigated the reliability of common building
energy simulation programs against the Twin Houses, extending Annex
58's experiments in scope (roller blinds, internal-gain schedules and
other fabric-related functionality); 42 institutes from 11 countries
participated in Annex 71; the recommended methodology is the two-phase
blind-then-open validation ("Blind phase" first, ideally by different
persons), as used in Annex 58 (Strachan et al., 2016) and Annex 71.
The report explicitly documents that Annex 58's two BES empirical
validation experiments preceded it and defines the specification for
the empirical validation experiment conducted on the Twin Houses,
including the datasets usable for model-parameter identification and
fault-detection research.

What this means for HARES: after BESTEST, the credible "next mountain"
is measured-data empirical validation on the Annex 58/71 Twin House
datasets, using the two-phase blind/open protocol. The datasets are
public (the Annex 71 BES-validation report specifies them; the reports
are freely downloadable from dynastee.info). This is exactly the
residential-dynamic-model reliability evidence a reviewer or a
national-lab user will ask for beyond comparative testing.

Can HARES run it today: no fixture family exists; the engine has the
needed physics surface (single-zone to two-zone free-running dynamics,
internal gains, blinds) but the datasets, the blind-phase protocol and
the reported metrics (hourly temperature traces, heating power) are
unbuilt. Row-shaped gap: a Twin-House fixture family with a blind-phase
run script and an open-phase reconciliation report.

### 1.6 OCHRE parity (already the house method)

Task 5.4 expands the OCHRE fixture assertions. External note: the
vendored OCHRE tree (vendors/OCHRE/ochre/utils/envelope.py, line 377)
carries a TODO "option to use ASHRAE140 method for film coefficients",
which confirms OCHRE itself is not ASHRAE 140-conformant by default;
HARES's dual anchor (140 bands for the envelope, OCHRE parity for
residential behavior) is the correct split and matches the charter's
correctness criterion.

---

## 2. The RL-for-buildings evaluation conventions

What an RL paper reviewer will demand from HARES's gym, with the
citeable conventions:

### 2.1 The KPI set

Source fetched: "Characterizing MARL for Energy Control: A Multi-KPI
Benchmark on the CityLearn Environment" (Khouja, Jendoubi, Mahjoub,
Mahfoudhi, De Kock, Singh, Formanek, InstaDeep; arXiv:2602.19223v2,
March 2026, https://arxiv.org/html/2602.19223v2), full text read. What
it says: the CityLearn KPI family, all defined so lower is better:
carbon emissions, total electricity consumption, ramping (absolute
change in consumption between consecutive timesteps), discomfort
proportion, energy cost, load factor, unserved energy, average daily
peak, peak; augmented with battery depth-of-discharge via rainflow
cycle counting and an agent-importance (Shapley-style) score. Reward
structuring convention: a per-timestep negative weighted sum
r = -(alpha*d + beta*e + gamma*ramp + lambda*solar) with d = thermal
discomfort, e = consumption, ramp = consecutive-step change, solar =
import minus on-site generation; the paper sets all four weights to
0.1 for equal, interpretable scale.

Also fetched: CityLearn v2 (Nweye et al., 2025,
https://escholarship.org/content/qt5t48x8xk/qt5t48x8xk.pdf) and the
CityLearn Challenge pages (https://www.citylearn.net/citylearn_challenge/2020.html)
confirming the challenge metrics lineage (ramping costs, peak demands,
carbon emissions), and the Sinergym paper (Campoy-Nieves et al.,
Energy and Buildings 115075, 2024; arXiv:2412.08293,
https://arxiv.org/abs/2412.08293) which positions benchmark packaging
(versioned environments, predefined benchmarks, replication support)
as the standard's core feature.

### 2.2 The comfort model and its applicability

Sources fetched: the public ASHRAE 55-2020 addendum PDF
(https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20addenda/55_2020_h_20221230.pdf):
the PMV method applies for average metabolic rates 1.0 to 4.0 met,
clothing insulation 0.0 to 1.5 clo, and air speeds within scope; the
sedentary simplified method is limited to 1.0 to 1.3 met and 0.5 to
1.0 clo (same limitation visible in the 55-2017 addendum and 55-2013
text, fetched at
https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20addenda/55_2017_d_20200731.pdf
and https://ierga.com/hr/wp-content/uploads/sites/2/2017/10/ASHRAE-55-2013.pdf).
The CBE Thermal Comfort Tool page (https://ashrae55.berkeley.edu/)
states the comfort boundary as PMV between -0.5 and +0.5 for the
standard's comfort zone (80 percent acceptability; 90 percent with the
tighter band, per USGBC's reference guide
https://www.usgbc.org/node/2758273).

Consequence for HARES: setpoint-deviation comfort metrics (CityLearn's
d(t)) are always computable from zone temperatures; PMV additionally
needs mean radiant temperature, humidity and assumed met/clo, and its
applicability bounds (1.0-4.0 met, 0.0-1.5 clo, air speed limits) must
be stated wherever a HARES reward component claims ASHRAE 55 lineage.
Row-shaped gap: the comfort contract in the RewardSpec (row C3) should
expose setpoint-degree-hours as the default component (defensible from
telemetry alone) and a PMV component only with MRT/humidity inputs and
an applicability note citing the 55 bounds.

- UNVERIFIABLE-WEB: ASHRAE 55-2020 normative Section 5 and Normative
  Appendix A (the exact PMV computation and elevated air-speed
  provisions). The addenda PDFs above establish the applicability
  numbers publicly; supply the normative section from library access
  before citing equation numbers in docs.

### 2.3 Reward normalization, evaluation protocol, and the episodes/second convention

Fetched sources and what they establish:

- Grid2Op reward conventions (https://grid2op.readthedocs.io/en/latest/user/reward.html,
  full page read): every reward derives from BaseReward and must define
  reward_min and reward_max, which the environment exposes as the gym
  reward_range; rewards are combinable via CombinedReward and
  CombinedScaledReward; multi-reward evaluation is supported through
  the info dict (named other_rewards); competition scoring is kept
  separate from the training reward (the reserved "__score" key). This
  is the citeable convention for HARES's RewardSpec (C3): each named
  component carries a declared range, composition is explicit, and the
  leaderboard score is never conflated with the reward.

- The CityLearn MARL benchmark paper (arXiv:2602.19223v2) evaluation
  protocol: independent hyperparameter tuning per algorithm, minimum 3
  seeds for selection, evaluation statistics reported as IQM
  (interquartile mean) and CVaR (conditional value at risk) with 95
  percent stratified bootstrap confidence intervals, probability of
  improvement for pairwise comparison, and sample-efficiency curves,
  citing Agarwal et al. 2022 (rliable) and Gorsane et al. 2022. What
  the reviewer will demand from HARES: the gym must make multi-seed,
  multi-episode evaluation cheap and the docs must show the IQM/CVaR
  recipe rather than single-run means.

- Episodes/second: the accepted convention in this literature is
  reported environment steps per second (or samples per second) pinned
  to named hardware, environment configuration and rollout settings.
  The MARL paper's identical-rollout-conditions statement is the
  protocol exemplar; Grid2Op's runner documentation separates
  environment configuration from agent evaluation for the same reason.
  HARES's C5 row already proposes a PPO-representative bench; the gap
  is the reporting template: env-steps/s with hardware, obs/action
  widths, episode length, number of envs and reward spec, stored with
  the run. Row-shaped gap: C5's acceptance should cite the
  CityLearn/Grid2Op reporting shape.

SINERGY/FLORIS-adjacent conventions, for completeness: the Sinergym
paper (arXiv:2412.08293) makes "predefined benchmarks, experiment
visualization and replication support" the deliverable, and NREL's
FLORIS shows the same discipline in an adjacent domain: a
controls-oriented engineering model whose credibility rests on
published comparison studies (Doekemeijer et al. 2022, NREL
"Comparison of the Gaussian Wind Farm Model with Measured Data",
https://docs.nlr.gov/docs/fy22osti/81375.pdf: "model validation remains
one of the largest barriers for commercial wind farm control"; and
Quick 2024, "Verification and Validation of Wind Farm Flow Models",
J. Phys. Conf. Ser. 2767, 092074,
https://orbit.dtu.dk/files/382748643/Quick_2024_J._Phys._Conf._Ser._2767_092074.pdf).
The transferable convention: a versioned benchmark suite plus published
comparison reports is what makes a simulator citable.

### 2.4 Rows that land the RL conventions (mapping)

- C3 (RewardSpec): adopt the Grid2Op declared-range convention and the
  CityLearn equal-weight default preset; cite both in the module docs.
- C5 (episodes/second): adopt the env-steps/s reporting template with
  hardware and configuration.
- C6 (gymnasium conformance): the env checker plus the supported-API
  statement mirrors Sinergym's replication-support claim.
- New gap rows are proposed in section 6 (V7, V8).

---

## 3. The grid-ops co-simulation conventions

### 3.1 HELICS federation design and time synchronization

Source fetched: HELICS documentation, "Timing Configuration"
(https://docs.helics.org/en/main/user-guide/fundamental_topics/timing_configuration.html),
full page read. What it says: the two fundamental roles are data
exchange and synchronized simulated time. The contract: each federate
requests a time (helicsFederateRequestTime) and is granted either the
requested time or an earlier valid time; being granted an earlier time
is always accompanied by a new value or message on one of its inputs.
The core guarantees no federate is asked to simulate a time in the past
(monotone grants). The configuration knobs, in seconds: period (grant
times forced to n*period + offset), offset, time_delta (minimum
interval from the last grant), uninterruptible (grants equal requests,
no nuisance wakes), wait_for_current_time_update (be last at a time so
all other federates' outputs are visible). Termination: all federates
granted HELICS_TIME_MAXTIME or all notify the broker; termination
cascades cores then brokers. The worked example (logger at 1 ms
uninterruptible, generator at 1 ms with 0.5 ms offset, event-driven
controller requesting MAXTIME with a 0.010 ms time_delta, power system
fixed 1 ms uninterruptible) is the federation-design pattern: fixed-
cadence physics federates plus event-driven controllers plus a lagged
logger.

Additional sources: "Co-simulation Architectures"
(https://docs.helics.org/en/main/user-guide/advanced_topics/architectures.html)
and the timing-optimization page
(https://docs.helics.org/en/main/user-guide/advanced_topics/timingOptimization.html)
covering when HELICS grants early based on the federates a federate
depends on; and the NREL paper Hardy et al. 2024, "HELICS: A
Co-Simulation Framework for Scalable Multi-Domain Modeling" (IEEE
Access 2024; NREL doc fy24osti/87610.pdf,
https://docs.nlr.gov/docs/fy24osti/87610.pdf): "Though a federate may
request a given time, HELICS may grant an earlier time if there is new
data on any of the input interfaces the federate has defined."

What this pins for HARES's rows:

- A1 (grid-signal channel): frequency and price arrive as inputs;
  under HELICS semantics an input arrival legitimately grants an early
  time. HARES's dwelling federate must define whether early grants
  step the engine or only stage inputs (A3's staleness contract).
- A3 (co-simulation contract): the doc's fixed-cadence statement
  should be written in HELICS terms: HARES federates declare period =
  engine step, time_delta = 0, uninterruptible = false only if early
  grants stage inputs without stepping; the "intermediate grants
  publish without stepping" behavior observed in dwelling.py is the
  HELICS-native grant-early rule, not a defect; the contract doc
  should say so.
- A8 (wheel watchdog): version-coupled surface; the HELICS docs track
  behavior changes between 2.x and 3.x (the 2.x timing page at
  https://docs.helics.org/en/helics2/user-guide/timing.html states the
  same two termination conditions), so the watchdog should feature-test
  against documented behavior, not wheel versions.

### 3.2 IEEE 1547-2018 trip and ride-through values

Structure and category semantics, sources fetched (PDF, downloaded and
text-extracted): Sandia PVPMC presentation "Smart Inverters and Grid
Support Requirements IEEE 1547-2018" (Aminul Karim,
https://pvpmc.sandia.gov/app/uploads/sites/243/2022/10/2-Aminul-Smart-Inverters-and-Grid-Support-Requirements.pdf),
which reproduces the EPRI-developed 1547-2018 ride-through figures.
What it says, in the standard's own terms:

- The response hierarchy: ride-through (permissive operation:
  DER may continue or cease at its discretion; mandatory operation:
  active and reactive current exchange required; momentary cessation:
  cease energizing, rapid recovery), restore output, trip, return to
  service. "Cease to energize" is at the point of DER connection and
  does not necessarily mean disconnection.
- Voltage ride-through structure per category (I, II, III): continuous
  operation 0.88 to 1.10 pu; the categories differ in how deep and how
  long they must ride. The deck's Category II figure shows
  ride-through-or-trip (permissive) zones above 1.10 pu and below
  0.88 pu with cumulative-duration boundaries in the 0.16 s to 21 s
  class (values as legible in the reproduced figure; the exact
  per-category rows are standard-text, see UNVERIFIABLE-WEB below),
  with Category III adding a momentary-cessation requirement and a
  longer zero-voltage ride-through requirement.
- Restore output: active current back to at least 80 percent of
  pre-disturbance within 0.4 s of voltage returning to the mandatory
  or continuous range (P0.4s >= 0.8 x Ppre x Vpost).
- Recurring disturbances: ride-through is defined over cumulative
  undervoltage/overvoltage duration within a disturbance set; a
  continuous-range period of 5 to 20 s (category dependent) resets the
  timer; 2 to 3 sets maximum depending on category; set counts reset
  after 20 to 60 minutes.
- Frequency ride-through and trip, all categories on one figure:
  continuous operation 58.8 to 61.2 Hz (default band, with an allowable
  settings envelope 57.0 to 61.8 Hz and a V/f <= 1.1 condition);
  mandatory operation bands on either side; may-ride-through-or-trip
  zones beyond; cumulative time is the sum of ride-through and trip
  clearing time. Slide values visible: 61.8 Hz and 58.8 Hz as
  mandatory-operation edges, 62.0 Hz and 56.5 Hz class edges at 0.16 s,
  299 to 300 s cumulative defaults near 61.2 and 58.5 Hz.

The shall-trip default settings, sources fetched (multiple independent
public utility sheets, all agreeing, quoting the standard's default
trip table): MISO "Guideline for IEEE Std 1547-2018 Implementation"
(https://cdn.misoenergy.org/MISO%20Guideline%20for%20IEEE%20Std%201547388042.pdf);
Unitil "Default IEEE 1547-2018 Setting Requirements"
(https://unitil.com/sites/default/files/2022-09/Default_IEEE1547-2018_Settings_Requirements_Issued.pdf);
FirstEnergy PA default inverter settings
(https://www.firstenergycorp.com/content/dam/feconnect/files/retail/pa/PA-IEEE-1547-2018-Specified-Inverter-Settings%20(003).pdf);
NYSEG/RG&E default smart inverter settings
(https://www.nyseg.com/documents/40132/5899056/NYSEG%2BRGE+Default+IEEE-1547+Smart+Inverter+Se_NYSEG+11.15.22.pdf).
The agreed defaults: OF2 62.0 Hz, 0.16 s; OF1 61.2 Hz, 300 s; UF1
58.5 Hz, 300 s (NERC's guideline notes the 300 s underfrequency
default is typically not set to trip; NRECA's guide lists UF1 58.5 Hz
with a 300 s trip time and notes coordination requirements);
UF2 56.5 Hz, 0.16 s. NRECA's "Guide to the IEEE 1547-2018 Standard"
(https://www.cooperative.com/programs-services/bts/documents/reports/nreca-guide-to-ieee-1547-2018-march-2019.pdf)
additionally states: the 2018 standard disallows voltage tripping
inside the continuous operating region and requires ride-through in
the mandatory regions. NREL's clause-by-clause summary
(https://docs.nlr.gov/docs/fy20osti/75184.pdf) confirms voltage
ride-through requirements for all categories are specified in the
standard's ride-through tables (tables 14-16 numbering per that
summary; the figures in the Sandia deck correspond).

- UNVERIFIABLE-WEB: the exact rows of IEEE 1547-2018 Tables 18 and 19
  (voltage and frequency ride-through performance categories: the
  per-category voltage windows, cumulative durations and clearing
  times; also Table 20/21 default and range settings). The figures and
  defaults above are publicly documented, but a row that pins trip
  tables as a compliance claim should have the standard's table text
  supplied from library access. Scope to supply: Tables 18, 19, and
  the default setting tables, plus Clause 8 testing references and
  Figure 1 (page 43, VRT thresholds and clearing times).

What HARES's Task 4.6 and the Axis A rows will implement, with these
citations:

- Task 4.6 (trip/ride-through layer): the shall-trip defaults
  (OF2/OF1/UF1/UF2 above), the 0.88 to 1.10 pu and 58.8 to 61.2 Hz
  continuous bands, per-category ride-through windows with cumulative
  duration accounting, the no-trip-inside-continuous rule, and the
  0.4 s / 80 percent restore-output behavior. The "no trip tables
  configured" warning stays (it is the 1547-2018 posture that these
  are configurable settings, not hardwired physics).
- A1 (frequency channel): the 59.3 Hz test event in A1's acceptance is
  inside the mandatory-operation band (58.5 to 58.8 Hz class), i.e.
  the DER must continue exchanging current there; the test vector set
  should include one point per region: 59.3 Hz (mandatory operation),
  61.5 Hz (mandatory), 62.0 Hz (trip, 0.16 s), 56.5 Hz (trip, 0.16 s),
  58.0 Hz (may ride through or trip).
- A4 (trip observability): per-DER state, trip time and
  time-to-reconnect map directly onto the standard's trip / return to
  service vocabulary; the restore-output 0.4 s rule is the reconnect
  test.
- A5 (load behavior below 0.9 pu): orthogonal to 1547 (which governs
  DER, not loads); cite load models for the ZIP/motor stall contract
  separately. No 1547 citation should appear in A5's docs.

### 3.3 IEEE 1459 and the power-quality surface

Sources fetched: IEEE SA pages for IEEE 1459-2010
(https://standards.ieee.org/standard/1459-2010.html: "definitions of
electric power to quantify the flow of electrical energy in
single-phase and three-phase circuits under sinusoidal, non-sinusoidal,
balanced, or unbalanced conditions") and the newer IEEE 1459-2025
edition (https://standards.ieee.org/ieee/1459/7578/, same scope
statement; a 2025 revision exists). Secondary: the Scribd-hosted
overview ("The IEEE Standard 1459: What and Why") noting 1459 defines
apparent power per phase with modifications for system asymmetry, and
the power-quality blog
(https://powerquality.blog/2022/10/18/power-network-parameters-standards-with-implements-ieee-1459-power-definitions/)
on 1459's billing-consistent power definitions.

Applicability to HARES: HARES's electrical surface is fundamental-frequency
and steady-state (the A7 service-entrance row's invariant S^2 = P^2 +
Q^2 per the axis spec, and per-leg RMS currents). Under sinusoidal,
single-frequency assumptions the classic definitions coincide with
1459's; 1459's distinctive content (effective apparent power Se under
unbalance, non-fundamental distortion power D, the separation of
harmonic power) becomes relevant only if HARES adds harmonics or
unbalance power-quality metrics. Decision recorded for the rows: A7
should document its S/P/Q definitions explicitly as the sinusoidal
special case of IEEE 1459-2010/2025, and any future THD or unbalance
metric row must adopt the 1459 definitions (Se, D, the effective
voltage/current concepts) rather than ad hoc forms.

- UNVERIFIABLE-WEB: IEEE 1459-2010 (and 1459-2025) full text. Scope to
  supply: Section 3 (definitions, effective apparent power), for the
  A7 docs row.

---

## 4. The Monte Carlo and occupancy literature

### 4.1 The OS-HPXML and ResStock stochastic occupancy family, provenance

Sources fetched:

- ResStock Technical Reference Guide 2025.1 (NatLabRockies,
  https://natlabrockies.github.io/ResStock.github.io/assets/trd/ResStockTechnicalReferenceGuide_2025_1.pdf),
  downloaded and text-extracted. What it says: "In ResStock, schedules
  are generated either using a stochastic occupancy-generator
  (inherited from OS-HPXML) or through more simplistic defined
  schedules"; the generator was "Developed using the American Time Use
  Survey (ATUS) data from 2013-2017, submetered appliance energy data,
  and a supplemental hot water model, the generator combines Markov
  chain and probability-sampling for schedule simulation"; it produces
  occupant activity schedules for seven activities (sleeping, personal
  hygiene, laundry, cooking, dish washing, absent, active-at-home),
  then combines them with appliance information into household-level
  appliance and hot-water schedules; a schedule table marks each
  column stochastic or non-stochastic (occupants, lighting_interior,
  clothes washer/dryer, cooking, dishwasher, hot water fixtures,
  ceiling fan, EV availability: stochastic; setpoints, exterior
  lighting, refrigerators: non-stochastic). Section 7 documents the
  conditional-distribution sampling over 150+ housing characteristics.

- OpenStudio-HPXML documentation, v1.8.1 workflow inputs
  (https://openstudio-hpxml.readthedocs.io/en/v1.8.1/workflow_inputs.html),
  fetched in full: the detailed-schedule CSV contract (column names,
  units, the MAX=1 normalization rule: schedules define when, not how
  much; annual totals multiply the schedule shape), the "Can Be
  Stochastically Generated" column list (matching HARES's
  hpxml_schedule.rs surface), the stochastic generator inputs
  (NumberofResidents, state, lat/lon, UTC offset), the DST spring-
  forward/fall-back CSV behavior, and the 301-2022 Addendum C default
  fractions for the non-stochastic path.

- The upstream residential occupancy lineage (fetched):
  Richardson, Thomson, Infield, "Domestic electricity use: a
  high-resolution energy demand model" (Energy and Buildings 42(10),
  2010; Loughborough University repository,
  https://repository.lboro.ac.uk/articles/journal_contribution/Domestic_electricity_use_a_high-resolution_energy_demand_model/9573941):
  the model combines patterns of active occupancy (a Markov-chain
  occupancy model from UK time-use data) with appliance and lighting
  event models at one-minute resolution. The 2026 review by Vogl et
  al. (Energy and Buildings,
  https://www.sciencedirect.com/science/article/pii/S0378778826005827)
  traces the line: "A prominent line of work starts with Richardson et
  al., who introduce a two-state occupancy model (present vs absent)
  for multiple occupants. McKenna et al. ..." (the CREST demand model
  v2.0 lineage at Loughborough). The ATUS-based successor used by
  OS-HPXML is the three-state model of Koupaei and Cetin, "Stochastic
  residential occupancy schedules based on the American Time Use
  Survey" (Semantic Scholar record
  https://www.semanticscholar.org/paper/68240d2183f71d2cc60571ccf53034f704f946ba;
  Iowa State dissertation "A Technique for Developing High-Resolution
  Residential...," 2019,
  https://dr.lib.iastate.edu/bitstreams/0499e055-9dfe-41d3-8023-46a0d574315d/download):
  a three-state stochastic occupancy model from the 2019 ATUS with
  weekday/weekend differences.

Interpretation for HARES: the mission's "Richardson-LMAS v2.0 lineage"
is the Richardson two-state Markov occupancy model (2008/2010, UK TUS)
carried forward through the Loughborough CREST demand model v2.0
(McKenna, Thomson), then re-estimated on US data by OS-HPXML's
stochastic schedule generator (initially Richardson-style, now the
ATUS 2013-2017 seven-activity Markov chain per the ResStock TRD). The
reference contract B1 cites should be: ResStock TRD 2025.1 Section 7
and the OS-HPXML schedule docs (the operational contract HARES's
generated-schedule family already mirrors), with Richardson 2010 and
Koupaei-Cetin as the method citations.

### 4.2 The NREL published variance references for B1's acceptance

Sources fetched:

- "End-Use Load Profiles for the U.S. Building Stock: Methodology and
  Results of Model Calibration, Validation, and Uncertainty
  Quantification" (Pigman, McCarty, et al., NREL, 2022,
  https://www.osti.gov/servlets/purl/1909353; also the 2021 Frick et
  al. companion NREL/TP-79108,
  https://docs.nrel.gov/docs/fy21osti/79108.pdf, and the DOE-hosted
  copy https://www.energy.gov/sites/default/files/2022-12/bto-eulp-120122.pdf):
  the EULP project's validation compared ResStock modeled monthly and
  annual consumption against EIA survey data (RECS/CBECS), with the
  validation effort focused on electricity and natural gas; this is
  the published calibration-and-uncertainty record the B1 row's
  fleet-aggregate acceptance should be measured against.

- The LBNL EULP webinar deck (Present and Frick, June 2021,
  https://emp.lbl.gov/sites/default/files/emp-files/2021.10.28_eulp_webinar.pdf)
  summarizing the residential stock model calibration and validation
  to the same effect.

Row-shaped gap: B1's acceptance says "the fleet's aggregate diversity
matches the ResStock reference within a stated tolerance". The
tolerance must be stated against a named published quantity: the EULP
report's distribution of per-home annual electricity by end use
(moments across the synthetic stock) is the reference; the row should
pin (a) the EULP report and figure/table numbers, (b) the comparison
statistic (KS distance or per-quantile bands), and (c) the ATUS
activity-set provenance (seven activities, 2013-2017) as a documented
contract with the OS-HPXML upstream commit hash.

- UNVERIFIABLE-WEB: none for this family; the TRD, the OS-HPXML docs
  and the EULP reports are all public. The ANSI/RESNET/ICC 301
  Addendum C text remains the only paywalled element (see 1.4).

---

## 5. Recent work (2024-2026) watch-list

1. Sinergym (Campoy-Nieves et al., Energy and Buildings 115075, 2024;
   arXiv:2412.08293, https://arxiv.org/abs/2412.08293): an open-source
   E+-based RL testbed whose contribution is benchmark packaging:
   predefined benchmarks, versioned environments, replication support.
   Watch and adopt: the benchmark-packaging pattern for HARES's gym
   (C6's version contract should include a citable benchmark DOI per
   release).

2. CityLearn 3.x (https://www.citylearn.net/, docs v3.0.2 fetched) and
   the CityLearn v2 paper (Nweye et al. 2025,
   https://escholarship.org/content/qt5t48x8xk/qt5t48x8xk.pdf): the
   moving reference for KPI definitions and district-scale RL
   evaluation. Watch: KPI drift between versions; HARES's RewardSpec
   presets should pin the KPI formulas by version.

3. "Characterizing MARL for Energy Control: A Multi-KPI Benchmark on
   the CityLearn Environment" (arXiv:2602.19223, March 2026): sets the
   current evaluation bar (IQM, CVaR, bootstrap CIs, probability of
   improvement, battery DoD via rainflow counting, agent importance).
   Watch and adopt: the stats protocol for any HARES RL benchmark
   claim.

4. NeuralBES (arXiv:2610.10459, October 2026,
   https://arxiv.org/html/2610.10459v1): a differentiable, control-
   aware building-energy emulator parameterizing a two-node RC model
   with a shared encoder, aimed at scalable building energy modeling.
   Watch: differentiable emulators are becoming the calibration-adjacent
   baseline; HARES's answer is its exact physics plus the calibration
   surface (Task 4.5, B7). Track its benchmark tasks for the
   calibration row's comparison set.

5. Physics-informed RL for building control with a differentiable
   thermodynamic constraint layer (Hedayat et al. 2025, Energies
   18(23):6310, https://www.mdpi.com/1996-1073/18/23/6310): comfort-
   constrained actor-critic with differentiable feasibility projection.
   Watch: the reward/constraint split HARES's RewardSpec (C3) should
   be able to express (hard comfort guards versus penalized comfort).

6. ASHRAE 140 evolution: "Update of ASHRAE Standard 140/BESTEST
   Thermal Fabric test cases" (Xin et al., IBPSA BS2021,
   https://publications.ibpsa.org/proceedings/bs/2021/papers/bs2021_30365.pdf):
   summarizes the work to update the thermal fabric test cases; the
   Better Building report shows the community now validates against
   140-2020 with the same case structure. Watch: the next edition's
   case set; HARES pins 140-2017 bands today and a reconciliation row
   is proposed (V2).

7. Verification and validation culture from adjacent domains: Quick
   2024 (https://orbit.dtu.dk/files/382748643/Quick_2024_J._Phys._Conf._Ser._2767_092074.pdf)
   and Doekemeijer et al. 2022 (https://docs.nlr.gov/docs/fy22osti/81375.pdf)
   for FLORIS: model validation named the largest barrier to
   commercial wind-farm control adoption; the remedy was published
   comparison studies against measured data. Watch: the same argument
   is HARES's charter case for Twin-House empirical validation (V4).

8. HELICS scale evidence: Hardy et al. 2024 (IEEE Access 2024,
   https://docs.nlr.gov/docs/fy24osti/87610.pdf): the canonical citation
   for HELICS's time contract; cite it in docs/cosim.md (A3) rather
   than folklore.

---

## 6. ROW PROPOSALS (for the schedule's next planning round)

Each entry: title, size (S about a day, M a few days, L about a week),
spec reference into this file, dependencies.

| # | title | size | spec | depends on |
|---|---|---|---|---|
| V1 | the ASHRAE 140 edition contract: reconcile the pinned 140-2017 bands against 140-2020 (the Better Building public report ranges), name the errata PDF as provenance, resolve the "AA/C0" shorthand into the standard's own case designations | S | 1.1, 1.2 | none |
| V2 | wire the missing 140 basic cases: 610, 620, 650 plus 920, 950, 960 with in-code bands and the night-ventilation/zero-deadband fixture contracts | M | 1.2 | I-08 Task 1.8 (as 5.2) |
| V3 | the HERS BESTEST L-series residential suite (L100A to L170A, L200A to L202A) as fixtures with the Annex B22-style acceptance procedure | L | 1.2 | V2; the geometry surface |
| V4 | the Twin-House empirical validation fixture family (Annex 58/71 datasets, two-phase blind/open protocol, hourly temperature and heating-power reconciliation) | L | 1.5 | V3 preferred, not hard |
| V5 | the ISO 13786 verification table (areal capacity, decrement factor, phase lag per construction) against Annex B/C values plus one public implementation | M | 1.3 | Task 5.2 |
| V6 | the RESNET 301 defaults pin: Addendum C schedule fractions and Addendum B emissions factors as golden tests, with the OS-HPXML docs page as public witness | S | 1.4 | none |
| V7 | the RL benchmark reporting kit: the C5 bench published in the CityLearn/Grid2Op shape (env-steps/s with hardware and configuration; IQM, CVaR, bootstrap CIs per the 2026 MARL benchmark protocol) | S | 2.1, 2.3 | C3, C5 |
| V8 | the comfort contract in the RewardSpec: setpoint-degree-hours default, PMV component only with MRT and humidity inputs and the ASHRAE 55 applicability note (1.0 to 4.0 met, 0.0 to 1.5 clo) | M | 2.2 | C3 |
| V9 | the IEEE 1547 default settings pack for Task 4.6: the four shall-trip defaults, continuous bands (0.88 to 1.10 pu, 58.8 to 61.2 Hz), per-category ride-through windows with cumulative duration, the 0.4 s and 80 percent restore rule, and one test vector per operating region | M | 3.2 | I-08 Task 4.6 |
| V10 | the service-entrance power-definitions note: A7's S, P, Q documented as the sinusoidal special case of IEEE 1459, with the future-quality-metrics boundary stated | S | 3.3 | A7 |
| V11 | the occupancy provenance contract: ATUS 2013-2017 seven-activity Markov chain, the OS-HPXML upstream hash, the Richardson 2010 and Koupaei-Cetin citations pinned in the schedule module's contract docs | S | 4.1 | B1 (or first, feeding B1) |
| V12 | the B1 variance acceptance made concrete: the EULP report's per-home end-use distributions as the reference, the comparison statistic and tolerance pinned | M | 4.2 | B1, B5 |
| V13 | the TM63 calibration vocabulary row: the fit reports adopt TM63's measurement-and-verification structure via the shared evaluation surface | S | 1.4 | Task 4.5, B7 |
| V14 | the external-audit sweep increment: watch-list items 1 to 8 checked against upstream heads, findings become rows (the charter's criterion 6 mechanism) | S | 5 | none; annual |

Sizes assume the I-08 rules (RED first, gates, review) apply.

End of record.
