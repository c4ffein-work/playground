from django.core.exceptions import ImproperlyConfigured
from django.test import SimpleTestCase

from config.checks import DEV_SECRET_KEY, validate_production_settings


class ProductionSettingsCheckTests(SimpleTestCase):
    """validate_production_settings runs at settings import; unit-test the logic."""

    def test_debug_true_allows_dev_secret(self):
        validate_production_settings(True, DEV_SECRET_KEY)  # no raise

    def test_debug_false_with_dev_secret_refuses_to_start(self):
        with self.assertRaises(ImproperlyConfigured):
            validate_production_settings(False, DEV_SECRET_KEY)

    def test_debug_false_with_empty_secret_refuses_to_start(self):
        with self.assertRaises(ImproperlyConfigured):
            validate_production_settings(False, "")

    def test_debug_false_with_real_secret_is_accepted(self):
        validate_production_settings(
            False, "a-genuinely-random-64-char-secret-key-XXXXXXXXXXXXXXXXXXXXXXXX"
        )  # no raise
