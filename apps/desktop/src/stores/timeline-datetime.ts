// Convert a datetime-local filter value into the UTC ISO string the
// timeline query contract compares lexicographically. `since` uses
// the start of the selected minute; `until` uses its end.
export function datetimeLocalToUtcIso(
  value: string,
  bound: 'start' | 'end' = 'start',
): string | undefined {
  if (!value) {
    return undefined
  }
  const instant = new Date(value)
  if (Number.isNaN(instant.getTime())) {
    return undefined
  }
  if (bound === 'end') {
    instant.setSeconds(59, 999)
  }
  return instant.toISOString()
}
