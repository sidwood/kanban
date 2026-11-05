"""Record one outcome per check and summarise them per test.

Outcomes:

- PASS: the behaviour matches Linear.
- FAIL: it does not. Any FAIL makes the run exit non-zero.
- XFAIL: a waived failure whose text is exactly the signature the
  waiver named. Any other failure is FAIL.
- XPASS: a waived check that now passes. It does not fail the run;
  the waiver can be removed.
- SKIP: the check cannot run here (for example, a write in read-only
  mode, or data the attached database does not hold).
"""

from .harness import redact_structure
from .harness import refusal_text as core_refusal_text

PASS = "PASS"
FAIL = "FAIL"
XFAIL = "XFAIL"
XPASS = "XPASS"
SKIP = "SKIP"
OUTCOMES = (PASS, FAIL, XFAIL, XPASS, SKIP)


def redact_nested(value, redact):
    """Replace complete secrets in strings before a value is rendered."""
    return redact_structure(value, redact)


def short(value, limit=160):
    """A one-line, bounded rendering of a value for a reason string."""
    if isinstance(value, (set, frozenset)):
        value = sorted(value, key=str)
    text = value if isinstance(value, str) else repr(value)
    text = " ".join(text.split()) if not isinstance(value, str) else text.replace("\n", "\\n")
    return text if len(text) <= limit else text[: limit - 3] + "..."


def first_difference(left, right):
    """Describe where two strings first differ."""
    for index, (a, b) in enumerate(zip(left, right)):
        if a != b:
            return f"first difference at offset {index}: {short(a)} vs {short(b)}"
    if len(left) != len(right):
        return f"lengths differ: {len(left)} vs {len(right)}"
    return "identical"


class Test:
    """The outcomes recorded under one numbered acceptance test."""

    def __init__(self, number, title):
        self.number = number
        self.title = title
        self.results = []

    def count(self, outcome):
        return sum(1 for result in self.results if result[1] == outcome)

    @property
    def verdict(self):
        passed = self.count(PASS) + self.count(XPASS) + self.count(XFAIL)
        failed = self.count(FAIL)
        if failed and passed:
            return "partial"
        if failed:
            return "fail"
        if passed:
            return "pass"
        return "skipped"


class Recorder:
    """Print each outcome as it lands and keep them for the summary."""

    def __init__(self, emit=print, redact=lambda text: text):
        self._emit = emit
        self._redact = redact
        self.tests = []
        self.current = None

    def begin(self, number, title):
        self.current = Test(number, title)
        self.tests.append(self.current)
        self._emit("")
        self._emit(self._redact(f"== Test {number}: {title}"))

    def _shown(self, value):
        """Redact raw strings before escaping, shortening, or a difference."""
        cleaned = redact_nested(value, self._redact)
        text = cleaned if isinstance(cleaned, str) else repr(cleaned)
        return self._redact(text)

    def _record(self, outcome, description, reason):
        test = self.current
        if test is None:
            raise RuntimeError("record a check inside a test")
        description = self._redact(description)
        if reason:
            reason = self._redact(str(reason))
        identifier = f"{test.number}.{len(test.results) + 1:02d}"
        test.results.append((identifier, outcome, description, reason))
        line = f"{outcome:<5} {identifier} {description}"
        if reason:
            line += f" -- {reason}"
        self._emit(self._redact(line))
        return outcome

    def check(self, description, ok, reason=""):
        """PASS when `ok`, otherwise FAIL."""
        self._record(PASS if ok else FAIL, description, reason)
        return bool(ok)

    def equal(self, description, actual, expected):
        """PASS when `actual == expected`; the reason shows both."""
        shown_actual = self._shown(actual)
        shown_expected = self._shown(expected)
        if actual == expected:
            return self.check(description, True, short(shown_actual))
        if isinstance(actual, str) and isinstance(expected, str):
            reason = first_difference(shown_actual, shown_expected)
            if reason == "identical":
                reason = "values differ"
        else:
            reason = f"expected {short(shown_expected)}, got {short(shown_actual)}"
        return self.check(description, False, reason)

    def xfail(self, description, ok, reason="", *, expected):
        """Waive only a failure whose reason is exactly `expected`.

        A pass is XPASS. A missing or empty signature does not waive,
        and neither does any other failure text.
        """
        if not isinstance(expected, str) or expected == "":
            self._record(FAIL, description, reason or "a waiver needs the expected failure")
            return False
        if ok:
            self._record(XPASS, description, reason)
            return True
        if reason == expected:
            self._record(XFAIL, description, reason)
            return False
        self._record(FAIL, description, reason)
        return False

    def skip(self, description, reason):
        self._record(SKIP, description, reason)

    def describe_result(self, result):
        """Render a tool result after removing secrets from its text."""
        return result.describe(self._redact)

    def redact_text(self, text):
        """Remove known secrets from text that is about to be shown."""
        return self._redact(text)

    def redact_value(self, value):
        """Remove known secrets from nested strings before formatting."""
        return redact_structure(value, self._redact)

    def refusal_text(self, error):
        """Core refusal text with secrets removed before it is formatted."""
        return core_refusal_text(error, self._redact)

    def abort(self, error):
        """A test stopped early; whatever it had not checked is a FAIL."""
        self._record(FAIL, "test stopped early", short(self._shown(str(error)), 300))

    def totals(self):
        totals = {outcome: 0 for outcome in OUTCOMES}
        for test in self.tests:
            for outcome in OUTCOMES:
                totals[outcome] += test.count(outcome)
        return totals

    def summary_lines(self):
        lines = ["", "== Summary"]
        lines.append(f"{'test':<5} {'verdict':<8} pass fail xfail xpass skip  title")
        for test in self.tests:
            lines.append(
                f"{test.number:<5} {test.verdict:<8} "
                f"{test.count(PASS):>4} {test.count(FAIL):>4} {test.count(XFAIL):>5} "
                f"{test.count(XPASS):>5} {test.count(SKIP):>4}  {test.title}"
            )
        totals = self.totals()
        lines.append(
            "total: "
            + ", ".join(f"{totals[outcome]} {outcome}" for outcome in OUTCOMES)
        )
        for outcome in (FAIL, XFAIL, XPASS):
            listed = [
                (identifier, description, reason)
                for test in self.tests
                for identifier, recorded, description, reason in test.results
                if recorded == outcome
            ]
            if listed:
                lines.append(f"{outcome}:")
                for identifier, description, reason in listed:
                    detail = f" -- {reason}" if reason else ""
                    lines.append(f"  {identifier} {description}{detail}")
        return [self._redact(line) for line in lines]

    def exit_code(self):
        return 1 if self.totals()[FAIL] else 0
