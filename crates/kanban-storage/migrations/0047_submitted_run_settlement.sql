UPDATE capabilities
SET status = 'settled', settled_at = coalesce(settled_at, unixepoch())
WHERE status = 'active'
  AND EXISTS (SELECT 1 FROM submissions WHERE capability_id = capabilities.id);

UPDATE dispatch_requests
SET completed_at = unixepoch(), version = version + 1
WHERE completed_at IS NULL
  AND EXISTS (
      SELECT 1 FROM runs JOIN submissions ON submissions.run_id = runs.id
      WHERE runs.dispatch_request_id = dispatch_requests.id
  );
