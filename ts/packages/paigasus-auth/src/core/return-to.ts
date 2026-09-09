// SPDX-License-Identifier: Apache-2.0

/**
 * Accept `raw` only as a same-origin relative path; otherwise return `fallback`.
 *
 * This is an OPEN REDIRECT surface. The rules, in order:
 *   - must be a string with no leading or trailing whitespace (a leading tab or space would
 *     otherwise let "\thttps://evil.com" past a naive startsWith check);
 *   - must contain no interior tab, LF or CR. A browser strips these characters while parsing
 *     a URL, so "/\t/evil.com" normalises to "//evil.com" — a protocol-relative, off-origin
 *     redirect. The Web `Headers` API rejects a header value containing CR or LF, but it
 *     accepts a bare TAB, so that API does not cover this character;
 *   - must start with "/";
 *   - must not start with "//" or "/\" — both are protocol-relative to a browser;
 *   - must contain no backslash anywhere, because browsers normalise "\" to "/" in URLs;
 *   - must not contain a percent-encoded slash or backslash, which a downstream decode would
 *     turn back into one of the cases above.
 */
export function validateReturnTo(raw: string | undefined | null, fallback: string): string {
  if (typeof raw !== 'string') return fallback;
  if (raw !== raw.trim() || raw.length === 0) return fallback;
  if (/[\t\n\r]/.test(raw)) return fallback;
  if (!raw.startsWith('/')) return fallback;
  if (raw.startsWith('//')) return fallback;
  if (raw.includes('\\')) return fallback;
  if (/%2f/i.test(raw.slice(1, 4)) || /%5c/i.test(raw)) return fallback;
  return raw;
}
