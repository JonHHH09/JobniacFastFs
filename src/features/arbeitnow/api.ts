import {
  type ArbeitnowCapabilitiesDto,
  type ArbeitnowImportResultDto,
  type ArbeitnowPageDto,
  type ArbeitnowScanRequestDto,
  commands,
} from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

/** Whether the Arbeitnow services are composed. Rust answers; a failed check reads as off (hidden). */
export function fetchArbeitnowCapabilities(): Promise<ArbeitnowCapabilitiesDto> {
  return runCommand(() => commands.arbeitnowCapabilitiesGet());
}

/** Scans the fixed board with the filters. Rust returns the first page with opaque row ids only. */
export function scanArbeitnow(request: ArbeitnowScanRequestDto): Promise<ArbeitnowPageDto> {
  return runCommand(() => commands.arbeitnowScan(request));
}

/** Loads the next page of the held scan. Rust keeps the cursor; nothing is sent. */
export function loadArbeitnowNextPage(): Promise<ArbeitnowPageDto> {
  return runCommand(() => commands.arbeitnowNextPage());
}

/** Imports one held row by its opaque id. The candidate token stays in Rust. */
export function importArbeitnowRow(rowId: string): Promise<ArbeitnowImportResultDto> {
  return runCommand(() => commands.arbeitnowImportRow(rowId));
}
