use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::routing::post;
use axum::{routing::get, Form, Router};
use indexmap::IndexMap;
use rand::distr::{Alphanumeric, SampleString};
use taskchampion::Uuid;
use std::collections::{HashMap, HashSet};
use std::env;
use std::string::ToString;
use taskwarrior_web::backend::task::{get_project_list, TaskOperation};
use taskwarrior_web::core::app::AppState;
use taskwarrior_web::core::errors::FormValidation;
use taskwarrior_web::endpoints::tasks::{self, change_task_status};
use taskwarrior_web::endpoints::tasks::task_query_builder::TaskQuery;
use taskwarrior_web::endpoints::tasks::{
    fetch_active_task, get_task_details, list_tasks, run_annotate_command,
    run_denotate_command, run_modify_command, task_add, task_undo, toggle_task_active, Task,
    TaskUUID, TaskViewDataRetType,
};
use taskwarrior_web::{
    task_query_merge_previous_params, task_query_previous_params, FlashMsg, NewTask, TWGlobalState,
    TaskActions, TEMPLATES,
};
use tera::Context;
use tracing::{error, info, trace};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    // initialize tracing
    init_tracing();
    match dotenvy::dotenv() {
        Ok(_) => {}
        Err(_) => {}
    };

    let addr = format!(
        "0.0.0.0:{}",
        env::var("TWK_SERVER_PORT").unwrap_or("3000".to_string())
    );

    let app_settings = AppState::default();

    // build our application with a route
    let app = Router::new()
        .route("/", get(front_page))
        .nest_service("/dist", tower_http::services::ServeDir::new("./dist"))
        .route("/tasks", get(tasks_display))
        .route("/tasks", post(do_task_actions))
        .route("/tasks/undo/report", get(get_undo_report))
        .route("/tasks/undo/confirmed", post(undo_last_change))
        .route("/tasks/add", get(display_task_add_window))
        .route("/tasks/active", get(get_active_task))
        .route("/tasks/add", post(create_new_task))
        .route("/msg", get(display_flash_message))
        .route("/msg_clr", get(just_empty))
        .route("/tag_bar", get(get_tag_bar))
        .route("/task_action_bar", get(get_task_action_bar))
        .route("/task_details", get(display_task_details))
        .route("/bars", get(get_bar))
        .with_state(app_settings);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("running on address: {}", addr);
    axum::serve(listener, app).await.unwrap();
}

fn get_default_context(state: &AppState) -> Context {
    state.into()
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "org_me=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_line_number(true))
        .init();
}

async fn display_task_details(
    Query(param): Query<HashMap<String, String>>,
    app_state: State<AppState>,
) -> Html<String> {
    let task_id = param.get("task_id").unwrap().clone();
    let task = get_task_details(task_id).unwrap();
    let tq = TaskQuery::new(TWGlobalState::default());
    let tasks = list_tasks(&tq).unwrap();
    let mut ctx = get_default_context(&app_state);
    ctx.insert("tasks_db", &tasks);
    ctx.insert("task", &task);
    Html(TEMPLATES.render("task_details.html", &ctx).unwrap())
}

async fn get_active_task(app_state: State<AppState>) -> Html<String> {
    let mut ctx = get_default_context(&app_state);
    if let Ok(v) = fetch_active_task() {
        if let Some(v) = v {
            ctx.insert("active_task", &v);
        }
    }
    Html(TEMPLATES.render("active_task.html", &ctx).unwrap())
}

async fn get_task_action_bar(app_state: State<AppState>) -> Html<String> {
    let ctx = get_default_context(&app_state);
    Html(TEMPLATES.render("task_action_bar.html", &ctx).unwrap())
}

async fn get_bar(
    Query(param): Query<HashMap<String, String>>,
    app_state: State<AppState>,
) -> Html<String> {
    if let Some(bar) = param.get("bar") {
        let ctx = get_default_context(&app_state);
        if bar == "left_action_bar" {
            Html(TEMPLATES.render("left_action_bar.html", &ctx).unwrap())
        } else {
            Html(TEMPLATES.render("task_action_bar.html", &ctx).unwrap())
        }
    } else {
        Html("".to_string())
    }
}

async fn get_tag_bar(app_state: State<AppState>) -> Html<String> {
    let ctx = get_default_context(&app_state);
    Html(TEMPLATES.render("tag_bar.html", &ctx).unwrap())
}

async fn just_empty() -> Html<String> {
    Html("".to_string())
}

async fn display_flash_message(
    Query(msg): Query<FlashMsg>,
    app_state: State<AppState>,
) -> Html<String> {
    let mut ctx = get_default_context(&app_state);
    ctx.insert("msg", &msg.msg());
    ctx.insert("timeout", &msg.timeout());
    Html(TEMPLATES.render("flash_msg.html", &ctx).unwrap())
}

async fn get_undo_report(app_state: State<AppState>) -> Html<String> {
    match taskwarrior_web::backend::task::get_undo_operations(&app_state.task_storage_path) {
        Ok(s) => {
            let mut ctx = get_default_context(&app_state);
            let number_operations: i64 = s.values().map(|f| f.len() as i64).sum();
            let heading = format!("The following {} operations would be reverted", number_operations);
            let undo_report: Vec<(_, &Vec<_>)> = s.iter().map(|(uuid, v)| {
                let task_description = match get_task_details(uuid.to_string()) {
                    Ok(t) => format!("{}({})", t.description, uuid),
                    Err(_) => uuid.to_string(),
                };
                (task_description, v)
            }).collect();
            ctx.insert("heading", &heading);
            ctx.insert("undo_report", &undo_report);
            Html(TEMPLATES.render("undo_report.html", &ctx).unwrap())
        }
        Err(e) => {
            let mut ctx = get_default_context(&app_state);
            ctx.insert("heading", &e.to_string());
            ctx.insert("undo_report", &HashMap::<Uuid, Vec<TaskOperation>>::new());
            Html(TEMPLATES.render("error.html", &ctx).unwrap())
        }
    }
}

async fn display_task_add_window(
    Query(params): Query<TWGlobalState>,
    app_state: State<AppState>,
) -> Html<String> {
    let tq: TaskQuery = params
        .filter_value()
        .clone()
        .and_then(|v| {
            if v == "" {
                Some(TaskQuery::default())
            } else {
                Some(serde_json::from_str(&v).unwrap_or(TaskQuery::default()))
            }
        })
        .or(Some(TaskQuery::default()))
        .unwrap();
    let project_list = get_project_list(&app_state.task_storage_path).unwrap_or(Vec::new());
    let mut ctx = get_default_context(&app_state);
    let new_task = NewTask::new(
        None,
        Some(tq.tags().join(" ")),
        tq.project().clone(),
        None,
        None,
    );
    ctx.insert("new_task", &new_task);
    ctx.insert("tags", &tq.tags().join(" "));
    ctx.insert("project", tq.project());
    ctx.insert("project_list", &project_list);
    ctx.insert("validation", &FormValidation::default());
    Html(TEMPLATES.render("task_add.html", &ctx).unwrap())
}

async fn undo_last_change(
    Query(params): Query<TWGlobalState>,
    app_state: State<AppState>,
) -> Html<String> {
    task_undo().unwrap();
    let fm = FlashMsg::new("Undo successful", None);
    get_tasks_view(task_query_previous_params(&params), Some(fm), &app_state)
}

fn make_shortcut(shortcuts: &mut HashSet<String>) -> String {
    let alpha = Alphanumeric::default();
    let mut len = 2;
    let mut tries = 0;
    loop {
        let shortcut = alpha.sample_string(&mut rand::rng(), len).to_lowercase();
        if !shortcuts.contains(&shortcut) {
            shortcuts.insert(shortcut.clone());
            return shortcut;
        }
        tries += 1;
        if tries > 1000 {
            len += 1;
            if len > 3 {
                panic!("too many shortcuts! this should not happen");
            }
            tries = 0;
        }
    }
}

fn get_tasks_view_data(
    mut tasks: IndexMap<TaskUUID, Task>,
    filters: &Vec<String>,
) -> TaskViewDataRetType {
    let mut tag_map: HashMap<String, String> = HashMap::new();
    let mut task_shortcut_map: HashMap<String, String> = HashMap::new();
    let mut shortcuts = HashSet::new();
    let task_list: Vec<Task> = tasks
        .values_mut()
        .map(|task| {
            if let Some(tags) = &mut task.tags {
                tags.iter_mut().for_each(|v| {
                    if !tasks::is_tag_keyword(v) {
                        *v = format!("+{}", v);
                    }
                    let shortcut = make_shortcut(&mut shortcuts);
                    tag_map.insert(v.clone(), shortcut);
                });
            }
            if let Some(project) = &task.project {
                // the project is not in the map, so all of it can be added
                if let None = tag_map.get(project) {
                    let parts: Vec<_> = project.split('.').collect();
                    let mut total_parts = vec![];
                    for part in parts {
                        total_parts.push(part);
                        let mut s = "project:".to_string();
                        s.push_str(&total_parts.join("."));
                        let shortcut = make_shortcut(&mut shortcuts);
                        tag_map.insert(s, shortcut);
                    }
                }
            }
            let shortcut = make_shortcut(&mut shortcuts);
            task_shortcut_map.insert(task.id.to_string(), shortcut);
            let shortcut = make_shortcut(&mut shortcuts);
            let uuid = task.uuid.clone();
            task_shortcut_map.insert(uuid, shortcut);
            task.clone()
        })
        .collect();
    for filter in filters {
        if !tag_map.contains_key(filter) {
            if tasks::is_tag_keyword(filter) {
            } else if tasks::is_a_tag(filter) {
                let ky = format!("@{}", filter);
                let shortcut = make_shortcut(&mut shortcuts);
                tag_map.insert(ky, shortcut);
            } else {
                let parts: Vec<_> = filter.split('.').collect();
                let mut total_parts = vec![];
                for part in parts {
                    total_parts.push(part);
                    let ky = format!("@{}", filter);
                    let shortcut = make_shortcut(&mut shortcuts);
                    tag_map.insert(ky, shortcut);
                }
            }
        }
    }

    TaskViewDataRetType {
        tasks,
        task_list,
        shortcuts,
        tag_map,
        task_shortcut_map,
    }
}

async fn front_page(app_state: State<AppState>) -> Html<String> {
    let tq = TaskQuery::new(TWGlobalState::default());
    let tasks = list_tasks(&tq).unwrap();
    let filters = tq.as_filter_text();
    let TaskViewDataRetType {
        tasks,
        tag_map,
        shortcuts: _,
        task_list,
        task_shortcut_map,
    } = get_tasks_view_data(tasks, &filters);
    let mut ctx = get_default_context(&app_state);
    ctx.insert("tasks_db", &tasks);
    ctx.insert("tasks", &task_list);
    ctx.insert("current_filter", &tq.as_filter_text());
    ctx.insert("filter_value", &serde_json::to_string(&tq).unwrap());
    ctx.insert("tags_map", &tag_map);
    ctx.insert("task_shortcuts", &task_shortcut_map);
    let t: Option<(&TaskUUID, &Task)> = tasks.iter().find(|(_, task)| task.start.is_some());
    if let Some((_, v)) = t {
        ctx.insert("active_task", v);
    }
    Html(TEMPLATES.render("base.html", &ctx).unwrap())
}

async fn tasks_display(
    Query(params): Query<TWGlobalState>,
    app_state: State<AppState>,
) -> Html<String> {
    get_tasks_view(task_query_merge_previous_params(&params), None, &app_state)
}

fn get_tasks_view(
    tq: TaskQuery,
    flash_msg: Option<FlashMsg>,
    app_state: &State<AppState>,
) -> Html<String> {
    Html(get_tasks_view_plain(tq, flash_msg, app_state))
}

fn get_tasks_view_plain(
    tq: TaskQuery,
    flash_msg: Option<FlashMsg>,
    app_state: &State<AppState>,
) -> String {
    let tasks = match list_tasks(&tq) {
        Ok(t) => t,
        Err(e) => {
            return e.to_string();
        }
    };
    let current_filter = tq.as_filter_text();
    let mut filter_ar = vec![];
    for filter in current_filter.iter() {
        if filter.starts_with("project:") {
            let mut stack = vec![];
            for part in filter.split(":").nth(1).unwrap().split(".") {
                stack.push(part);
                filter_ar.push(format!("project:{}", stack.join(".")))
            }
        } else {
            filter_ar.push(filter.to_string());
        }
    }
    let TaskViewDataRetType {
        tasks,
        tag_map,
        shortcuts: _,
        task_list,
        task_shortcut_map,
    } = get_tasks_view_data(tasks, &filter_ar);
    trace!("{:?}", tag_map);
    let mut ctx_b = get_default_context(&app_state);
    ctx_b.insert("tasks_db", &tasks);
    ctx_b.insert("tasks", &task_list);
    ctx_b.insert("current_filter", &filter_ar);
    ctx_b.insert("filter_value", &serde_json::to_string(&tq).unwrap());
    ctx_b.insert("tags_map", &tag_map);
    ctx_b.insert("task_shortcuts", &task_shortcut_map);
    if let Some(msg) = flash_msg {
        ctx_b.insert("has_toast", &true);
        ctx_b.insert("toast_msg", msg.msg());
        ctx_b.insert("toast_timeout", &msg.timeout());
    }
    let t = tasks.iter().find(|(_, task)| task.start.is_some());
    if let Some((_, v)) = t {
        ctx_b.insert("active_task", v);
    }
    TEMPLATES.render("tasks.html", &ctx_b).unwrap()
}

async fn create_new_task(
    app_state: State<AppState>,
    Form(new_task): Form<NewTask>,
) -> Response<String> {
    let s = if let Some(tw_q) = new_task.filter_value() {
        serde_json::from_str(tw_q).unwrap()
    } else {
        TaskQuery::default()
    };
    match task_add(&new_task, &app_state) {
        Ok(_) => {
            let flash_msg = FlashMsg::new("New task created", None);
            Response::builder()
                .status(StatusCode::CREATED)
                .header("HX-Retarget", "#list-of-tasks")
                .header("HX-Reswap", "innerHTML")
                .header("Content-Type", "text/html")
                .body(get_tasks_view_plain(s, Some(flash_msg), &app_state))
                .unwrap()
        }
        Err(e) => {
            let project_list = get_project_list(&app_state.task_storage_path).unwrap_or(Vec::new());
            let mut ctx = get_default_context(&app_state);
            ctx.insert("new_task", &new_task);
            ctx.insert("project_list", &project_list);
            ctx.insert("validation", &e);
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .body(TEMPLATES.render("task_add.html", &ctx).unwrap())
                .unwrap()
        }
    }
}

async fn do_task_actions(
    app_state: State<AppState>,
    Form(multipart): Form<TWGlobalState>,
) -> Html<String> {
    info!("{:?}", multipart);
    let fm = match multipart.action().clone().unwrap() {
        TaskActions::StatusUpdate => {
            if let Some(task) = taskwarrior_web::from_task_to_task_update(&multipart) {
                match change_task_status(task.clone(), &app_state) {
                    Ok(_) => FlashMsg::new(&format!("Task [{}] was updated", task.uuid), None),
                    Err(e) => {
                        error!("Failed: {}", e);
                        FlashMsg::new(&format!("Failed to update task: {e}"), None)
                    }
                }
            } else {
                FlashMsg::new("No task to update", None)
            }
        }
        TaskActions::ToggleTimer => {
            let task_uuid = multipart.uuid().clone().unwrap();
            match toggle_task_active(task_uuid, &app_state) {
                Ok(v) => {
                    if v {
                        FlashMsg::new(
                            &format!(
                                "Task {} started, any other tasks running were stopped",
                                task_uuid
                            ),
                            None,
                        )
                    } else {
                        FlashMsg::new(&format!("Task {} stopped", task_uuid), None)
                    }
                }
                Err(e) => {
                    error!("Failed: {}", e);
                    FlashMsg::new(&format!("Failed to update task: {e}"), None)
                }
            }
        }
        TaskActions::ModifyTask => {
            let cmd = multipart.task_entry().clone().unwrap();
            if cmd.is_empty() {
                error!("Failed: No annotation provided");
                FlashMsg::new("Failed to execute command, none provided", None)
            } else {
                match run_modify_command(multipart.uuid().unwrap(), &cmd) {
                    Ok(_) => FlashMsg::new("Modify command success", None),
                    Err(e) => FlashMsg::new(&format!("Modify command failed: {}", e), None),
                }
            }
        }
        TaskActions::AnnotateTask => {
            let cmd = multipart.task_entry().clone().unwrap();
            if cmd.is_empty() {
                error!("Failed: No command provided");
                FlashMsg::new("Failed to execute command, none provided", None)
            } else {
                match run_annotate_command(multipart.uuid().unwrap(), &cmd) {
                    Ok(_) => FlashMsg::new("Annotation added", None),
                    Err(e) => FlashMsg::new(&format!("Annotation command failed: {}", e), None),
                }
            }
        }
        TaskActions::DenotateTask => match run_denotate_command(multipart.uuid().unwrap()) {
            Ok(_) => FlashMsg::new("Denotated task", None),
            Err(e) => FlashMsg::new(&format!("Denotation command failed: {}", e), None),
        },
    };
    get_tasks_view(task_query_previous_params(&multipart), Some(fm), &app_state)
}
