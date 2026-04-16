"""Trigram-based locale detection for LibraryWriteEvent emission (ADR-042).

Detects the dominant language of a text segment using trigram frequency
analysis. Returns an ISO 639-1 language code ("en", "fr") or "" if
confidence is below the threshold.

No external dependencies — profiles are bundled inline.
"""

from __future__ import annotations

import math
from collections import Counter

# Confidence threshold per ADR-042: below this, return "" (empty string)
# which triggers English fallback in the librarian.
# Cosine similarity on trigram profiles typically yields 0.2-0.5 for correct
# matches; we use a relatively low absolute threshold and also require a
# minimum margin over the runner-up language for disambiguation.
CONFIDENCE_THRESHOLD = 0.15

# Minimum text length for reliable detection (trigrams need at least a few words)
MIN_TEXT_LENGTH = 20


def detect_locale(text: str) -> str:
    """Detect the locale of the given text using trigram frequency analysis.

    Args:
        text: The text to classify.

    Returns:
        ISO 639-1 language code ("en", "fr") or "" if confidence is below
        threshold or text is too short.
    """
    if not text or len(text.strip()) < MIN_TEXT_LENGTH:
        return ""

    text_trigrams = _compute_trigram_frequencies(text.lower())
    if not text_trigrams:
        return ""

    scores: list[tuple[str, float]] = []
    for lang, profile in _LANGUAGE_PROFILES.items():
        score = _cosine_similarity(text_trigrams, profile)
        scores.append((lang, score))

    scores.sort(key=lambda x: -x[1])

    if not scores or scores[0][1] < CONFIDENCE_THRESHOLD:
        return ""

    # Require minimum margin over runner-up for confident detection
    if len(scores) > 1:
        margin = scores[0][1] - scores[1][1]
        if margin < 0.03:
            return ""

    return scores[0][0]


def _compute_trigram_frequencies(text: str) -> dict[str, float]:
    """Compute normalized trigram frequency distribution for text."""
    # Normalize: lowercase, replace non-alpha with space, collapse spaces
    normalized = ""
    for ch in text:
        if ch.isalpha():
            normalized += ch
        else:
            if normalized and normalized[-1] != " ":
                normalized += " "

    trigrams = Counter()
    for i in range(len(normalized) - 2):
        tri = normalized[i : i + 3]
        trigrams[tri] += 1

    total = sum(trigrams.values())
    if total == 0:
        return {}

    return {tri: count / total for tri, count in trigrams.items()}


def _cosine_similarity(a: dict[str, float], b: dict[str, float]) -> float:
    """Compute cosine similarity between two frequency distributions."""
    # Only compute over keys present in both
    dot = 0.0
    for key in a:
        if key in b:
            dot += a[key] * b[key]

    mag_a = math.sqrt(sum(v * v for v in a.values()))
    mag_b = math.sqrt(sum(v * v for v in b.values()))

    if mag_a == 0.0 or mag_b == 0.0:
        return 0.0

    return dot / (mag_a * mag_b)


# ── Language Profiles ──────────────────────────────────────────────────
# Top trigrams for English and French, computed from representative corpora.
# These are normalized frequency values (sum to ~1.0 for top entries).
# Only the most discriminative trigrams are kept for efficiency.

_ENGLISH_PROFILE: dict[str, float] = {
    " th": 0.037, "the": 0.036, "he ": 0.025, "nd ": 0.018,
    "ing": 0.017, " an": 0.016, "and": 0.016, "ed ": 0.015,
    " in": 0.014, "ion": 0.014, "er ": 0.013, "tio": 0.013,
    "ati": 0.012, " of": 0.012, "of ": 0.011, "on ": 0.011,
    "tha": 0.010, "hat": 0.010, " to": 0.010, "to ": 0.010,
    " is": 0.009, "is ": 0.009, "ent": 0.009, " co": 0.009,
    "re ": 0.009, " re": 0.008, "for": 0.008, " fo": 0.008,
    "or ": 0.008, "ter": 0.008, " it": 0.007, "it ": 0.007,
    "al ": 0.007, "nt ": 0.007, " ha": 0.007, "has": 0.007,
    " wi": 0.007, "wit": 0.007, "ith": 0.007, "th ": 0.007,
    "not": 0.006, " no": 0.006, "ot ": 0.006, " wa": 0.006,
    "was": 0.006, "as ": 0.006, " be": 0.006, "all": 0.006,
    "ver": 0.006, "ons": 0.006, "her": 0.006, "his": 0.006,
    " he": 0.006, "est": 0.005, "ome": 0.005, "men": 0.005,
    "are": 0.005, " ar": 0.005, "ess": 0.005, "ive": 0.005,
}

_FRENCH_PROFILE: dict[str, float] = {
    " de": 0.030, "de ": 0.028, "es ": 0.025, " le": 0.022,
    "le ": 0.020, "ent": 0.019, " la": 0.018, "la ": 0.017,
    "les": 0.016, " les": 0.012, "ion": 0.015, "on ": 0.014,
    " qu": 0.014, "que": 0.014, "ue ": 0.013, "re ": 0.013,
    " et": 0.012, "et ": 0.012, " un": 0.012, "ons": 0.011,
    "tio": 0.011, "ati": 0.011, " pa": 0.011, "par": 0.010,
    "des": 0.010, " des": 0.008, "ns ": 0.010, "ne ": 0.009,
    " en": 0.009, "en ": 0.009, " co": 0.009, "men": 0.009,
    "eme": 0.008, "ait": 0.008, " po": 0.008, "pou": 0.008,
    "our": 0.008, "ur ": 0.008, " se": 0.008, "se ": 0.007,
    " ce": 0.007, " il": 0.007, "il ": 0.007, "est": 0.007,
    " est": 0.006, "ait": 0.007, "ant": 0.007, " da": 0.007,
    "dan": 0.007, "ans": 0.007, " su": 0.006, "sur": 0.006,
    "ter": 0.006, " pr": 0.006, "pre": 0.006, "pas": 0.006,
    " pas": 0.005, "com": 0.006, "une": 0.006, " une": 0.005,
}

_LANGUAGE_PROFILES: dict[str, dict[str, float]] = {
    "en": _ENGLISH_PROFILE,
    "fr": _FRENCH_PROFILE,
}
