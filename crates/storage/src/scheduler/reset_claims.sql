UPDATE scheduled_jobs SET state='scheduled',updated_at_ms=?1 WHERE state='claimed';
UPDATE queue_messages SET state='ready',attempts=attempts+1,available_at_ms=MAX(enqueued_at_ms,?1+250),
  claim_batch_id=NULL,consumer_id=NULL,consumer_generation=NULL WHERE state='claimed';
DELETE FROM queue_delivery_batches;
UPDATE cron_runs SET state=CASE WHEN attempt<?2+1 AND dispatch_deadline_at_ms>?1+250 THEN 'ready' ELSE 'failed' END,
  next_attempt_at_ms=CASE WHEN attempt<?2+1 AND dispatch_deadline_at_ms>?1+250 THEN ?1+250 ELSE NULL END,
  claimed_at_ms=NULL,last_unknown_reason=COALESCE(last_unknown_reason,'transport-timeout'),
  error_code=CASE WHEN dispatch_deadline_at_ms<=?1+250 THEN 'CRON_DISPATCH_DEADLINE'
    WHEN attempt>=?2+1 THEN 'CRON_RETRY_EXHAUSTED' ELSE error_code END,
  completed_at_ms=CASE WHEN attempt<?2+1 AND dispatch_deadline_at_ms>?1+250 THEN NULL ELSE ?1 END
WHERE state='claimed';
INSERT OR IGNORE INTO queue_dlq_pending
  (message_id,source_queue_id,target_queue_id,target_queue_generation,terminal_attempts,next_attempt_at_ms,created_at_ms,last_error_code)
SELECT m.id,m.queue_id,c.dlq_queue_id,c.dlq_queue_generation,m.attempts,?1+250,?1,'QUEUE_DLQ_BACKPRESSURED'
FROM queue_messages m JOIN queue_consumer_state c ON c.queue_id=m.queue_id
WHERE m.state='ready' AND m.attempts>c.max_retries AND c.dlq_queue_id IS NOT NULL;
DELETE FROM queue_messages WHERE state='ready' AND EXISTS(SELECT 1 FROM queue_consumer_state c
  WHERE c.queue_id=queue_messages.queue_id AND queue_messages.attempts>c.max_retries AND c.dlq_queue_id IS NULL);
