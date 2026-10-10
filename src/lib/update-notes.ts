/** Keep release changes readable; packaging metadata remains in the verified update payload. */
export function displayReleaseNotes(notes: string, portableHint: string): string {
  return notes.split(/\r?\n/)
    .filter(line => !/^\s*(?:Commit|SHA-256):\s*`?[a-f0-9]{7,64}`?\s*$/i.test(line))
    .map(line => line.trim() === 'Windows standalone EXE. Download and run directly.' ? portableHint : line)
    .join('\n').replace(/\n{3,}/g, '\n\n').trim();
}
