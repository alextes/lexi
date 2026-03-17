use crate::ai_interaction::tools::beacon_slot_check::BeaconNode;
use crate::db::Db;
use crate::scheduler::executor::execute_scheduled_job;
use crate::scheduler::types::ScheduledJob;
use crate::scheduler::SchedulerContext;
use crate::telegram;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{error, info, warn};

/// Tracks when a job was last executed
struct JobState {
    job: ScheduledJob,
    last_run: Option<DateTime<Utc>>,
}

/// Runs the scheduler service that manages and executes scheduled jobs.
/// Uses a polling approach to check cron schedules and execute jobs.
pub async fn run_scheduler<D, B>(ctx: SchedulerContext<D, B>) -> Result<()>
where
    D: Db + Clone + Send + Sync + 'static,
    B: BeaconNode + Clone + Send + Sync + 'static,
{
    info!("starting scheduler service");

    // Track job states
    let mut job_states: HashMap<i32, JobState> = HashMap::new();

    // Initial job load
    if let Err(e) = reload_jobs(&ctx.db, &mut job_states).await {
        error!(error = %e, "failed initial job load");
    }

    info!(
        job_count = job_states.len(),
        "scheduler started, checking jobs every minute"
    );

    let mut reload_counter = 0u32;

    // Main scheduler loop - check every minute
    loop {
        let now = Utc::now();

        // Check each job to see if it should run
        for state in job_states.values_mut() {
            if should_run_job(&state.job, state.last_run, now) {
                info!(
                    job_name = %state.job.name,
                    job_id = state.job.id,
                    "executing scheduled job"
                );

                // Execute the job
                if let Err(e) = execute_scheduled_job(&state.job, &ctx).await {
                    error!(
                        job_name = %state.job.name,
                        error = %e,
                        "scheduled job execution failed"
                    );

                    // Notify Telegram about the failure
                    let error_msg = format!("⚠️ Scheduled job '{}' failed:\n{}", state.job.name, e);
                    if let Err(send_err) = telegram::send_message(
                        &ctx.http_client,
                        &ctx.api_base_url,
                        &ctx.bot_token,
                        state.job.telegram_chat_id,
                        &error_msg,
                        state.job.message_thread_id,
                    )
                    .await
                    {
                        error!(
                            job_name = %state.job.name,
                            error = %send_err,
                            "failed to send error notification to Telegram"
                        );
                    }
                }

                // Update last run time regardless of success/failure to prevent retry loops
                state.last_run = Some(now);
            }
        }

        // Wait for next minute
        tokio::time::sleep(Duration::from_secs(60)).await;

        // Reload jobs from database every 5 minutes
        reload_counter += 1;
        if reload_counter >= 5 {
            reload_counter = 0;
            if let Err(e) = reload_jobs(&ctx.db, &mut job_states).await {
                error!(error = %e, "failed to reload scheduled jobs");
            }
        }
    }
}

/// Reload jobs from database, preserving last_run times for existing jobs
async fn reload_jobs<D: Db>(db: &D, job_states: &mut HashMap<i32, JobState>) -> Result<()> {
    info!("reloading scheduled jobs from database");

    let enabled_jobs = db.get_enabled_scheduled_jobs().await?;
    let enabled_job_ids: std::collections::HashSet<i32> =
        enabled_jobs.iter().map(|j| j.id).collect();

    // Remove jobs that are no longer enabled
    job_states.retain(|id, _| enabled_job_ids.contains(id));

    // Add or update jobs
    for job in enabled_jobs {
        if let Some(state) = job_states.get_mut(&job.id) {
            // Update job definition but keep last_run
            state.job = job;
        } else {
            // New job
            info!(
                job_id = job.id,
                job_name = %job.name,
                cron = %job.cron_schedule,
                "registered new scheduled job"
            );
            job_states.insert(
                job.id,
                JobState {
                    job,
                    last_run: None,
                },
            );
        }
    }

    info!(
        registered_count = job_states.len(),
        "scheduled jobs reload complete"
    );
    Ok(())
}

/// Check if a job should run based on its cron schedule
fn should_run_job(job: &ScheduledJob, last_run: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    // Parse the cron schedule
    let schedule = match croner::Cron::new(&job.cron_schedule).parse() {
        Ok(s) => s,
        Err(e) => {
            warn!(
                job_name = %job.name,
                cron = %job.cron_schedule,
                error = %e,
                "failed to parse cron schedule"
            );
            return false;
        }
    };

    // Find the next scheduled time after last_run (or epoch if never run)
    let after = last_run.unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());

    // Get the next occurrence after the last run
    match schedule.find_next_occurrence(&after, false) {
        Ok(next_run) => {
            // If the next scheduled time is in the past (or now), we should run
            next_run <= now
        }
        Err(_) => false,
    }
}
