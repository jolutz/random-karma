import csv

input_file = "/home/jonas/workspace/random_karma/src/cars.csv"
output_file = "/home/jonas/workspace/random_karma/src/cars.csv"  # Overwrite the same file

with open(input_file, "r", newline="") as infile:
    reader = csv.reader(infile)
    rows = list(reader)

# Find the header row (the one containing 'Vehicle' and 'Lap Time (m:ss.000)')
header_row = None
for i, row in enumerate(rows):
    if "Vehicle" in row and "Lap Time (m:ss.000)" in row:
        header_row = i
        break

if header_row is None:
    raise Exception("Could not find header row with Vehicle and Lap Time (m:ss.000)")

vehicle_idx = rows[header_row].index("Vehicle")
laptime_idx = rows[header_row].index("Lap Time (m:ss.000)")

# Extract only vehicle and lap time columns, skipping any non-data rows
out_rows = [["vehicle", "lap_time"]]
for row in rows[header_row + 1 :]:
    if len(row) > max(vehicle_idx, laptime_idx):
        vehicle = row[vehicle_idx].strip()
        lap_time = row[laptime_idx].strip()
        if vehicle and lap_time:
            out_rows.append([vehicle, lap_time])

with open(output_file, "w", newline="") as outfile:
    writer = csv.writer(outfile)
    writer.writerows(out_rows)

print(f"Successfully transformed {output_file}")
