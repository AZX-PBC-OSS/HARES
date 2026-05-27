#!/usr/bin/env python3
"""
Derive distinct weekend schedule fractions from ANSI/RESNET/ICC 301-2022
weekday fractions using the ASHRAE 90.2 / HERS Reference Home occupancy
assumption: weekdays occupied 5 PM--8 AM, weekends occupied all day.

Methodology:
1. For `occupants`, create a weekend profile by filling the weekday
   9 AM--5 PM dip to match the average of "home" hours (0-8, 17-23),
   then renormalize to sum = 1.0.

2. For derived occupancy-driven schedules (lighting, appliances, etc.),
   scale each hourly fraction by the weekend/weekday occupancy ratio:
     weekend[h] = weekday[h] * (occupants_weekend[h] / occupants_weekday[h])
   then renormalize to sum = 1.0.

This couples appliance/lighting usage to the occupancy pattern --
higher occupancy at a given hour means higher usage of those loads.
"""

import csv
import sys
from copy import deepcopy

# CSV columns: Schedule Name, Element, OCHRE Name, OCHRE Element, Values, Data Source

def parse_csv(path):
    rows = []
    with open(path, newline='') as f:
        reader = csv.reader(f)
        header = next(reader)
        for row in reader:
            if len(row) >= 6:
                rows.append({
                    'schedule_name': row[0].strip(),
                    'element': row[1].strip(),
                    'ochre_name': row[2].strip(),
                    'ochre_element': row[3].strip(),
                    'values': row[4].strip(),
                    'data_source': row[5].strip(),
                })
    return rows

def parse_fractions(s):
    return [float(x.strip()) for x in s.split(',') if x.strip()]

def format_fractions(arr):
    return ', '.join(f'{v:.3f}' for v in arr)

def sum_to_1(arr):
    """Check that fractions sum to ~1.0"""
    return abs(sum(arr) - 1.0) < 0.01

def compute_occupants_weekend(weekday):
    """
    ASHRAE 90.2/HERS Reference Home: occupancy 5 PM--8 AM weekdays, all day weekends.

    Weekday "home" hours:  0-8 (9 hours) and 17-23 (7 hours) = 16 hours
    Weekday "away" hours:   9-16 (8 hours)

    Weekend: fill the away dip by replacing away-hour fractions with the
    average of home-hour fractions, then renormalize to sum=1.0.
    """
    home_hours = list(range(0, 9)) + list(range(17, 24))
    away_hours = list(range(9, 17))

    home_vals = [weekday[h] for h in home_hours]
    home_avg = sum(home_vals) / len(home_vals)

    weekend = list(weekday)
    for h in away_hours:
        weekend[h] = home_avg

    total = sum(weekend)
    weekend = [v / total for v in weekend]

    assert sum_to_1(weekend), f"weekend sum={sum(weekend):.4f}"
    return weekend

def derive_from_occupancy(weekday, occupant_weekday, occupant_weekend):
    """
    Scale each hourly fraction by the weekend/weekday occupancy ratio,
    then renormalize.
    """
    weekend = []
    for h in range(24):
        ratio = occupant_weekend[h] / occupant_weekday[h] if occupant_weekday[h] > 0 else 1.0
        weekend.append(weekday[h] * ratio)

    total = sum(weekend)
    weekend = [v / total for v in weekend]

    assert sum_to_1(weekend), f"weekend sum={sum(weekend):.4f}"
    return weekend


def main():
    csv_path = sys.argv[1] if len(sys.argv) > 1 else 'defaults/Default Schedule Parameters.csv'
    rows = parse_csv(csv_path)

    # Build lookup: ochre_name -> weekday fractions
    weekday_by_name = {}
    for row in rows:
        if row['ochre_element'] == 'weekday_fractions':
            key = row['ochre_name']
            vals = parse_fractions(row['values'])
            if len(vals) == 24:
                weekday_by_name[key] = vals

    # Compute occupants weekend
    occ_name = 'Occupancy'
    occ_weekday = weekday_by_name[occ_name]
    occ_weekend = compute_occupants_weekend(occ_weekday)

    # For display
    print("=== occupants (Occupancy) weekday ===")
    print(format_fractions(occ_weekday))
    print(f"sum = {sum(occ_weekday):.4f}")
    print()
    print("=== occupants (Occupancy) weekend (derived) ===")
    print(format_fractions(occ_weekend))
    print(f"sum = {sum(occ_weekend):.4f}")
    print()

    # Derived schedule names (OCHRE names used in CSV)
    derived_names = [
        'Indoor Lighting',
        'Exterior Lighting',
        'Garage Lighting',
        'Water Heating',          # hot_water_fixtures
        'Cooking Range',
        'Dishwasher',
        'Clothes Washer',
        'Clothes Dryer',
        'MELs',                  # plug_loads_other
        'TV',                     # plug_loads_tv
        'Ceiling Fan',           # also occupancy-driven
        'Gas Grill',             # also occupancy-driven (weekend grilling!)
    ]

    for name in derived_names:
        wd = weekday_by_name.get(name)
        if wd is None:
            print(f"  WARNING: no weekday data for '{name}' -- skipping")
            continue
        we = derive_from_occupancy(wd, occ_weekday, occ_weekend)

        print(f"=== {name} weekday ===")
        print(format_fractions(wd))
        print(f"sum = {sum(wd):.4f}")
        print()
        print(f"=== {name} weekend (derived) ===")
        print(format_fractions(we))
        print(f"sum = {sum(we):.4f}")
        print()

    # Verify all derived weekend fractions differ from weekday
    print("=== Verification: at least one hour differs between weekday and weekend ===")
    all_pass = True
    for name in ['Occupancy'] + derived_names:
        wd = weekday_by_name.get(name)
        if wd is None:
            continue
        if name == 'Occupancy':
            we = occ_weekend
        else:
            we = derive_from_occupancy(wd, occ_weekday, occ_weekend)
        differs = any(abs(wd[h] - we[h]) > 1e-6 for h in range(24))
        status = "PASS" if differs else "FAIL"
        if not differs:
            all_pass = False
        max_diff = max(abs(wd[h] - we[h]) for h in range(24))
        print(f"  {name}: {status} (max hourly diff = {max_diff:.6f})")
    print()
    if all_pass:
        print("All schedules have distinct weekday/weekend fractions.")
    else:
        print("SOME SCHEDULES STILL HAVE IDENTICAL WEEKDAY/WEEKEND FRACTIONS!")

if __name__ == '__main__':
    main()
