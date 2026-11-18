from buggy_pkg.app import TokenBucket, parse_config, slow_sum


def test_slow_sum_matches_sum():
    assert slow_sum(10) == sum(range(10))


def test_parse_config(tmp_path):
    cfg = tmp_path / "app.conf"
    cfg.write_text("a=1\nb=2\n")
    assert parse_config(str(cfg)) == {"a": "1", "b": "2"}


def test_bucket_allows_after_refill():
    bucket = TokenBucket(rate=10, capacity=2, tokens=[])
    bucket.refill()
    assert bucket.allow(1) is True
