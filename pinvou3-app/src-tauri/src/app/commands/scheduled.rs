use super::prelude::*;
use crate::features::scheduled::tasks as scheduled_domain;
use scheduled_domain::*;

async_command_passthrough!(scheduled_domain, list_scheduled_tasks(state: State<'_, ScheduledTaskState>) -> Result<Vec<ScheduledTaskDto>, String>);
async_command_passthrough!(scheduled_domain, read_scheduled_task(id: String, state: State<'_, ScheduledTaskState>) -> Result<ScheduledTaskDetailDto, String>);
async_command_passthrough!(scheduled_domain, list_scheduled_task_runs(id: String, limit: Option<usize>, state: State<'_, ScheduledTaskState>) -> Result<Vec<ScheduledRunDto>, String>);
async_command_passthrough!(scheduled_domain, list_scheduled_runs(state: State<'_, ScheduledTaskState>) -> Result<Vec<ScheduledRunDto>, String>);
async_command_passthrough!(scheduled_domain, create_scheduled_task(input: CreateScheduledTaskInput, state: State<'_, ScheduledTaskState>) -> Result<ScheduledTaskDto, String>);
async_command_passthrough!(scheduled_domain, update_scheduled_task(id: String, input: UpdateScheduledTaskInput, state: State<'_, ScheduledTaskState>) -> Result<ScheduledTaskDto, String>);
async_command_passthrough!(scheduled_domain, pause_scheduled_task(id: String, state: State<'_, ScheduledTaskState>) -> Result<ScheduledTaskDto, String>);
async_command_passthrough!(scheduled_domain, resume_scheduled_task(id: String, state: State<'_, ScheduledTaskState>) -> Result<ScheduledTaskDto, String>);
async_command_passthrough!(scheduled_domain, set_scheduled_task_pinned(id: String, pinned: bool, state: State<'_, ScheduledTaskState>) -> Result<ScheduledTaskDto, String>);
async_command_passthrough!(scheduled_domain, delete_scheduled_task(id: String, state: State<'_, ScheduledTaskState>) -> Result<DeletedScheduledTaskDto, String>);
async_command_passthrough!(scheduled_domain, run_scheduled_task_now(id: String, state: State<'_, ScheduledTaskState>) -> Result<ScheduledRunDto, String>);
async_command_passthrough!(scheduled_domain, mark_scheduled_run_viewed(automation_id: String, run_id: String, state: State<'_, ScheduledTaskState>) -> Result<ScheduledRunViewedDto, String>);
sync_command_passthrough!(scheduled_domain, scheduled_task_chat_prompt() -> Result<String, String>);
