-- A browser print station leases the sheet it is printing, so a second tab or device never
-- prints the same sheet, and a station that disappears frees it again after the lease runs out.
ALTER TABLE print_jobs ADD COLUMN lease_by TEXT;
ALTER TABLE print_jobs ADD COLUMN lease_until TEXT;
