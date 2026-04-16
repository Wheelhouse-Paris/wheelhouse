"""Tests for trigram-based locale detection (Story 14.1.1, AC-5)."""

from __future__ import annotations

from wheelhouse.librarian.locale import detect_locale


def test_detect_english():
    """AC-5: English text returns 'en'."""
    text = (
        "The quick brown fox jumps over the lazy dog. "
        "This is a test of the English language detection system. "
        "It should correctly identify this text as English."
    )
    assert detect_locale(text) == "en"


def test_detect_french():
    """AC-5: French text returns 'fr'."""
    text = (
        "Le petit prince est un roman philosophique. "
        "Il raconte les aventures d'un jeune garcon. "
        "C'est un des livres les plus traduits au monde."
    )
    assert detect_locale(text) == "fr"


def test_empty_text_returns_empty():
    """AC-5: Empty string returns ''."""
    assert detect_locale("") == ""


def test_short_text_returns_empty():
    """AC-5: Very short text returns '' (below MIN_TEXT_LENGTH)."""
    assert detect_locale("Hi") == ""
    assert detect_locale("Bonjour") == ""


def test_low_confidence_returns_empty():
    """AC-5: Ambiguous/mixed text below confidence threshold returns ''."""
    # Numbers and symbols have no language signal
    assert detect_locale("12345 67890 !@#$% ^&*()")  == ""


def test_whitespace_only_returns_empty():
    """Whitespace-only text returns ''."""
    assert detect_locale("   \n\t  ") == ""


def test_longer_english_passage():
    """Longer English passage detected correctly."""
    text = (
        "In the beginning, there was nothing but an empty void. "
        "Then the universe expanded rapidly in what scientists call "
        "the Big Bang. This theory explains the origin of everything "
        "we observe in the cosmos today, from galaxies to stars to "
        "the fundamental particles that make up all matter."
    )
    assert detect_locale(text) == "en"


def test_longer_french_passage():
    """Longer French passage detected correctly."""
    text = (
        "La France est un pays situe en Europe occidentale. "
        "Elle est connue pour sa culture, sa gastronomie et son histoire. "
        "Paris, la capitale, est une des villes les plus visitees au monde. "
        "Le pays possede une grande diversite de paysages, des montagnes "
        "des Alpes aux plages de la Cote d'Azur."
    )
    assert detect_locale(text) == "fr"
