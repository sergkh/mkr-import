use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const SCOPES: &str = "https://www.googleapis.com/auth/calendar";
const CONFIG_DIR: &str = "config";
const CONFIG_PATH: &str = "config/config.json";
const CREDENTIALS_PATH: &str = "config/credentials.json";

#[derive(Debug, Parser)]
#[command(name = "mkr-import")]
#[command(about = "Sync MKR schedule to Google Calendar")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {

    Create, /// Add a new user
    Remove, /// Remove an existing user    
    Sync, /// Sync all users once (default)
    /// Sync continuously at intervals
    Watch {
        /// Interval format: 1h, 30m, 1h30m, or milliseconds
        #[arg(long)]
        interval: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    #[serde(default)]
    users: Vec<UserConfig>,
    #[serde(default = "default_time_zone", rename = "timeZone")]
    time_zone: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct UserConfig {
    #[serde(rename = "calendarId")]
    calendar_id: String,
    #[serde(rename = "chairId")]
    chair_id: i64,
    #[serde(rename = "teacherId")]
    teacher_id: i64,
    token: OAuthToken,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OAuthToken {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    expiry_date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    installed: InstalledCredentials,
}

#[derive(Debug, Deserialize)]
struct InstalledCredentials {
    client_id: String,
    client_secret: String,
    redirect_uris: Vec<String>,
    auth_uri: Option<String>,
    token_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MkrEvent {
    start: String,
    end: String,
    name: String,
    group: String,
    place: Option<String>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct CalendarInsertEvent {
    summary: String,
    location: Option<String>,
    description: String,
    start: CalendarDateTime,
    end: CalendarDateTime,
}

#[derive(Debug, Serialize)]
struct CalendarDateTime {
    #[serde(rename = "dateTime")]
    date_time: String,
    #[serde(rename = "timeZone")]
    time_zone: String,
}

#[derive(Debug, Deserialize)]
struct CalendarEventListResponse {
    items: Option<Vec<CalendarEvent>>,
}

#[derive(Debug, Deserialize)]
struct CalendarEvent {
    id: Option<String>,
    summary: Option<String>,
    location: Option<String>,
    start: Option<CalendarEventDateTime>,
    end: Option<CalendarEventDateTime>,
    #[serde(rename = "htmlLink")]
    html_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CalendarEventDateTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    ensure_config_dir()?;
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Sync) {
        Command::Create => create_command().await,
        Command::Remove => remove_command(),
        Command::Sync => sync_command().await,
        Command::Watch { interval } => watch_command(interval).await,
    }
}

fn default_time_zone() -> String {
    "Europe/Kyiv".to_string()
}

fn ensure_config_dir() -> Result<()> {
    if !Path::new(CONFIG_DIR).exists() {
        fs::create_dir_all(CONFIG_DIR).context("failed to create config directory")?;
    }
    Ok(())
}

fn load_config() -> Result<Config> {
    if !Path::new(CONFIG_PATH).exists() {
        return Ok(Config {
            users: Vec::new(),
            time_zone: default_time_zone(),
        });
    }

    let content = fs::read_to_string(CONFIG_PATH).context("failed to read config file")?;
    let config: Config = serde_json::from_str(&content).context("invalid config.json")?;
    Ok(config)
}

fn save_config(config: &Config) -> Result<()> {
    let payload = serde_json::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(CONFIG_PATH, payload).context("failed to write config file")?;
    Ok(())
}

fn load_credentials() -> Result<InstalledCredentials> {
    if !Path::new(CREDENTIALS_PATH).exists() {
        bail!("credentials.json not found. Please register a new Desktop app on https://console.cloud.google.com/ and download the credentials.json file.");
    }

    let content = fs::read_to_string(CREDENTIALS_PATH).context("failed to read credentials file")?;
    let file: CredentialsFile = serde_json::from_str(&content).context("invalid credentials.json")?;
    Ok(file.installed)
}

fn question(query: &str) -> Result<String> {
    print!("{query}");
    io::stdout().flush().context("failed to flush stdout")?;

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read input")?;
    Ok(line.trim().to_string())
}

fn auth_uri(credentials: &InstalledCredentials) -> &str {
    credentials
        .auth_uri
        .as_deref()
        .unwrap_or("https://accounts.google.com/o/oauth2/v2/auth")
}

fn token_uri(credentials: &InstalledCredentials) -> &str {
    credentials
        .token_uri
        .as_deref()
        .unwrap_or("https://oauth2.googleapis.com/token")
}

fn build_auth_url(credentials: &InstalledCredentials) -> Result<String> {
    let redirect_uri = credentials
        .redirect_uris
        .first()
        .ok_or_else(|| anyhow!("No redirect URI found in credentials"))?;

    let mut url = reqwest::Url::parse(auth_uri(credentials)).context("invalid auth URI")?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("client_id", &credentials.client_id);
        pairs.append_pair("redirect_uri", redirect_uri);
        pairs.append_pair("response_type", "code");
        pairs.append_pair("scope", SCOPES);
        pairs.append_pair("access_type", "offline");
        pairs.append_pair("prompt", "consent");
    }
    Ok(url.into())
}

fn extract_code_from_redirect_url(value: &str) -> Result<String> {
    if let Ok(url) = reqwest::Url::parse(value) {
        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
            .ok_or_else(|| anyhow!("Authorization code not found in redirect URL"))?;
        Ok(code)
    } else {
        Ok(value.to_string())
    }
}

async fn exchange_code_for_token(client: &Client, credentials: &InstalledCredentials, code: &str) -> Result<OAuthToken> {
    let redirect_uri = credentials
        .redirect_uris
        .first()
        .ok_or_else(|| anyhow!("No redirect URI found in credentials"))?
        .to_string();

    let params = [
        ("code", code.to_string()),
        ("client_id", credentials.client_id.clone()),
        ("client_secret", credentials.client_secret.clone()),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code".to_string()),
    ];

    let response = client
        .post(token_uri(credentials))
        .form(&params)
        .send()
        .await
        .context("failed to request OAuth token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("OAuth token exchange failed ({status}): {body}");
    }

    let data: OAuthTokenResponse = response
        .json()
        .await
        .context("failed to parse OAuth token response")?;

    Ok(OAuthToken {
        access_token: data.access_token,
        token_type: data.token_type,
        scope: data.scope,
        refresh_token: data.refresh_token,
        expires_in: data.expires_in,
        expiry_date: data
            .expires_in
            .map(|s| Utc::now().timestamp_millis() + (s * 1000)),
    })
}

async fn refresh_access_token(client: &Client, credentials: &InstalledCredentials, token: &OAuthToken) -> Result<OAuthToken> {
    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("Missing refresh_token; re-run create command"))?;

    let params = [
        ("client_id", credentials.client_id.clone()),
        ("client_secret", credentials.client_secret.clone()),
        ("refresh_token", refresh_token.clone()),
        ("grant_type", "refresh_token".to_string()),
    ];

    let response = client
        .post(token_uri(credentials))
        .form(&params)
        .send()
        .await
        .context("failed to refresh OAuth token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("OAuth refresh failed ({status}): {body}");
    }

    let data: OAuthTokenResponse = response
        .json()
        .await
        .context("failed to parse OAuth refresh response")?;

    Ok(OAuthToken {
        access_token: data.access_token,
        token_type: data.token_type.or_else(|| token.token_type.clone()),
        scope: data.scope.or_else(|| token.scope.clone()),
        refresh_token: data.refresh_token.or_else(|| token.refresh_token.clone()),
        expires_in: data.expires_in,
        expiry_date: data
            .expires_in
            .map(|s| Utc::now().timestamp_millis() + (s * 1000)),
    })
}

fn token_expired(token: &OAuthToken) -> bool {
    let Some(expiry_ms) = token.expiry_date else {
        return false;
    };
    Utc::now().timestamp_millis() + 60_000 >= expiry_ms
}

async fn ensure_access_token(client: &Client, credentials: &InstalledCredentials, token: &OAuthToken) -> Result<(String, Option<OAuthToken>)> {
    if token_expired(token) && token.refresh_token.is_some() {
        let refreshed = refresh_access_token(client, credentials, token).await?;
        return Ok((refreshed.access_token.clone(), Some(refreshed)));
    }

    if token.access_token.is_empty() {
        bail!("Missing access token; re-run create command");
    }

    Ok((token.access_token.clone(), None))
}

async fn create_command() -> Result<()> {
    let client = Client::new();
    let credentials = load_credentials()?;
    let mut config = load_config()?;

    println!("\n=== Creating New User ===\n");

    let calendar_id = question("Create a new Calendar at Google Calendar https://calendar.google.com/. Click on the calendar you want to use and copy its ID. \nEnter Calendar ID: ")?;
    if calendar_id.is_empty() {
        bail!("Calendar ID is required");
    }

    let chair_id_str = question("Chair ID can be obtained from the MKR site if you open the developer console and inspect the request data for the teacher page.\nEnter Chair ID: ")?;
    let chair_id: i64 = chair_id_str.parse().context("Invalid Chair ID")?;

    let teacher_id_str = question("Teacher ID can be obtained from the MKR site if to open developer console and check the request data for the teacher page.\nEnter Teacher ID: ")?;
    let teacher_id: i64 = teacher_id_str.parse().context("Invalid Teacher ID")?;

    if config.users.iter().any(|u| u.calendar_id == calendar_id) {
        bail!("Calendar ID {calendar_id} already exists");
    }

    println!("\nGetting OAuth token...");
    let auth_url = build_auth_url(&credentials)?;
    println!("\nAuthorize this app by visiting this URL:\n{auth_url}\n");
    let redirect_url = question("Enter the redirectURL after authorization here: ")?;
    let code = extract_code_from_redirect_url(&redirect_url)?;
    if code.is_empty() {
        bail!("Authorization code is required");
    }

    let token = exchange_code_for_token(&client, &credentials, &code).await?;

    config.users.push(UserConfig {
        calendar_id: calendar_id.clone(),
        chair_id,
        teacher_id,
        token,
    });
    save_config(&config)?;

    println!("\n✅ User created successfully!");
    println!("   Calendar ID: {calendar_id}");
    println!("   Chair ID: {chair_id}");
    println!("   Teacher ID: {teacher_id}");
    Ok(())
}

fn remove_command() -> Result<()> {
    let mut config = load_config()?;

    if config.users.is_empty() {
        println!("No users configured.");
        return Ok(());
    }

    println!("\n=== Remove User ===\n");
    println!("Current users:");
    for (idx, user) in config.users.iter().enumerate() {
        println!(
            "  {}. Calendar ID: {}, Chair ID: {}, Teacher ID: {}",
            idx + 1,
            user.calendar_id,
            user.chair_id,
            user.teacher_id
        );
    }
    println!();

    let answer = question("Enter the number of the user to remove (or \"cancel\"): ")?;
    if answer.eq_ignore_ascii_case("cancel") {
        println!("Cancelled.");
        return Ok(());
    }

    let index: usize = answer.parse::<usize>().context("Invalid selection")?;
    if index == 0 || index > config.users.len() {
        bail!("Invalid selection");
    }

    let removed = config.users.remove(index - 1);
    save_config(&config)?;

    println!(
        "\n✅ Removed user: Calendar ID {}, Chair ID {}, Teacher ID {}",
        removed.calendar_id,
        removed.chair_id,
        removed.teacher_id
    );
    Ok(())
}

fn parse_schedule_datetime(value: &str) -> Result<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M")
        .with_context(|| format!("Invalid schedule datetime: {value}"))?;

    let local_dt = Local
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| anyhow!("Ambiguous or invalid local datetime: {value}"))?;
    Ok(local_dt.with_timezone(&Utc))
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn events_equal(existing: &CalendarEvent, incoming: &CalendarInsertEvent) -> bool {
    let existing_start = existing
        .start
        .as_ref()
        .and_then(|s| s.date_time.as_ref())
        .and_then(|v| parse_rfc3339_utc(v));
    let existing_end = existing
        .end
        .as_ref()
        .and_then(|e| e.date_time.as_ref())
        .and_then(|v| parse_rfc3339_utc(v));
    let incoming_start = parse_rfc3339_utc(&incoming.start.date_time);
    let incoming_end = parse_rfc3339_utc(&incoming.end.date_time);

    existing.summary.as_deref().unwrap_or_default() == incoming.summary
        && existing.location.as_deref().unwrap_or_default()
            == incoming.location.as_deref().unwrap_or_default()
        && existing_start == incoming_start
        && existing_end == incoming_end
}

fn build_calendar_events_url(calendar_id: &str) -> String {
    format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events",
        urlencoding::encode(calendar_id)
    )
}

fn date_to_local_start_end(date: NaiveDate) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let day_start_local = Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .single()
        .ok_or_else(|| anyhow!("Invalid local date start: {date}"))?;

    let next_day = date + ChronoDuration::days(1);
    let next_day_start_local = Local
        .with_ymd_and_hms(
            next_day.year(),
            next_day.month(),
            next_day.day(),
            0,
            0,
            0,
        )
        .single()
        .ok_or_else(|| anyhow!("Invalid local date boundary: {next_day}"))?;

    let day_end_local = next_day_start_local - ChronoDuration::milliseconds(1);
    Ok((
        day_start_local.with_timezone(&Utc),
        day_end_local.with_timezone(&Utc),
    ))
}

async fn fetch_mkr_events(client: &Client, chair_id: i64, teacher_id: i64, start_date: NaiveDate, end_date: NaiveDate) -> Result<Vec<MkrEvent>> {
    let url = format!(
        "https://mkr.sergkh.com/structures/0/chairs/{chair_id}/teachers/{teacher_id}/schedule?startDate={}&endDate={}",
        start_date.format("%Y-%m-%d"),
        end_date.format("%Y-%m-%d")
    );

    let response = client
        .get(url)
        .send()
        .await
        .context("failed to fetch MKR schedule")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("MKR API failed ({status}): {body}");
    }

    response
        .json::<Vec<MkrEvent>>()
        .await
        .context("failed to parse MKR schedule response")
}

fn build_new_items(day_events: &[MkrEvent], time_zone: &str) -> Result<Vec<CalendarInsertEvent>> {
    day_events
        .iter()
        .map(|item| {
            let start = parse_schedule_datetime(&item.start)?.to_rfc3339();
            let end = parse_schedule_datetime(&item.end)?.to_rfc3339();

            Ok(CalendarInsertEvent {
                summary: format!("{} ({})", item.name, item.group),
                location: item.place.clone(),
                description: format!(
                    "Type: {}",
                    item.event_type
                        .as_deref()
                        .unwrap_or("unknown")
                ),
                start: CalendarDateTime {
                    date_time: start,
                    time_zone: time_zone.to_string(),
                },
                end: CalendarDateTime {
                    date_time: end,
                    time_zone: time_zone.to_string(),
                },
            })
        })
        .collect()
}

async fn list_calendar_events(
    client: &Client,
    access_token: &str,
    calendar_id: &str,
    day_start: DateTime<Utc>,
    day_end: DateTime<Utc>,
) -> Result<Vec<CalendarEvent>> {
    let endpoint = build_calendar_events_url(calendar_id);
    let response = client
        .get(endpoint)
        .bearer_auth(access_token)
        .query(&[
            ("timeMin", day_start.to_rfc3339()),
            ("timeMax", day_end.to_rfc3339()),
            ("singleEvents", "true".to_string()),
            ("orderBy", "startTime".to_string()),
        ])
        .send()
        .await
        .context("failed to list calendar events")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Calendar list failed ({status}): {body}");
    }

    let payload: CalendarEventListResponse = response
        .json()
        .await
        .context("failed to parse calendar list response")?;

    Ok(payload.items.unwrap_or_default())
}

async fn delete_calendar_event(client: &Client, access_token: &str, calendar_id: &str, event_id: &str) -> Result<()> {
    let endpoint = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
        urlencoding::encode(calendar_id),
        urlencoding::encode(event_id)
    );

    let response = client
        .delete(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .context("failed to delete calendar event")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Calendar delete failed ({status}): {body}");
    }

    Ok(())
}

async fn insert_calendar_event(
    client: &Client,
    access_token: &str,
    calendar_id: &str,
    event: &CalendarInsertEvent,
) -> Result<Option<String>> {
    let endpoint = build_calendar_events_url(calendar_id);
    let response = client
        .post(endpoint)
        .bearer_auth(access_token)
        .json(event)
        .send()
        .await
        .context("failed to insert calendar event")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Calendar insert failed ({status}): {body}");
    }

    let payload: CalendarEvent = response
        .json()
        .await
        .context("failed to parse calendar insert response")?;
    Ok(payload.html_link)
}

async fn sync_schedule_events_for_user(
    client: &Client,
    access_token: &str,
    user: &UserConfig,
    time_zone: &str,
) -> Result<()> {
    let start_date = Local::now().date_naive();
    let end_date = start_date + ChronoDuration::weeks(2);

    println!(
        "\n📋 Syncing for Calendar ID: {}, Teacher ID: {}",
        user.calendar_id, user.teacher_id
    );

    let events = fetch_mkr_events(client, user.chair_id, user.teacher_id, start_date, end_date).await?;
    println!("Fetched {} events", events.len());

    let mut grouped: BTreeMap<NaiveDate, Vec<MkrEvent>> = BTreeMap::new();
    let mut day = start_date;
    while day <= end_date {
        grouped.insert(day, Vec::new());
        day += ChronoDuration::days(1);
    }

    for event in events {
        let dt = NaiveDateTime::parse_from_str(&event.start, "%Y-%m-%d %H:%M")
            .with_context(|| format!("Invalid event start datetime: {}", event.start))?;
        let date = dt.date();
        if let Some(list) = grouped.get_mut(&date) {
            list.push(event);
        }
    }

    for (date, day_events) in grouped {
        println!("\n📅 Syncing {} ({} events)", date, day_events.len());
        let new_items = build_new_items(&day_events, time_zone)?;

        let (day_start, day_end) = date_to_local_start_end(date)?;
        let existing_items =
            list_calendar_events(client, access_token, &user.calendar_id, day_start, day_end).await?;

        let identical = existing_items.len() == new_items.len()
            && existing_items
                .iter()
                .zip(new_items.iter())
                .all(|(existing, incoming)| events_equal(existing, incoming));

        if identical {
            println!("✅ {} unchanged, skipping", date);
            continue;
        }
        println!("🔄 {} has changes, updating...", date);

        if !existing_items.is_empty() {
            println!("🗑 Removing {} old events", existing_items.len());
            for event in &existing_items {
                if let Some(event_id) = event.id.as_deref() {
                    if let Err(err) = delete_calendar_event(client, access_token, &user.calendar_id, event_id).await {
                        eprintln!("❌ Error deleting event: {err}");
                    }
                }
            }
        }

        for event in &new_items {
            match insert_calendar_event(client, access_token, &user.calendar_id, event).await {
                Ok(Some(link)) => println!("✅ Created: {} -> {}", event.summary, link),
                Ok(None) => println!("✅ Created: {}", event.summary),
                Err(err) => eprintln!("❌ Error creating event: {err}"),
            }
        }
    }

    Ok(())
}

async fn sync_command() -> Result<()> {
    let client = Client::new();
    let credentials = load_credentials()?;
    let mut config = load_config()?;

    if config.users.is_empty() {
        bail!("No users configured. Use \"create\" command to add a user.");
    }

    println!("Syncing {} user(s)...", config.users.len());
    let mut changed_tokens = false;

    for user in &mut config.users {
        match ensure_access_token(&client, &credentials, &user.token).await {
            Ok((access_token, maybe_updated_token)) => {
                if let Some(updated) = maybe_updated_token {
                    user.token = updated;
                    changed_tokens = true;
                }

                if let Err(err) =
                    sync_schedule_events_for_user(&client, &access_token, user, &config.time_zone).await
                {
                    eprintln!("Error syncing Teacher ID {}: {err}", user.teacher_id);
                } else {
                    println!("\n✅ Completed sync for Teacher ID: {}\n", user.teacher_id);
                }
            }
            Err(err) => {
                eprintln!("Error syncing Teacher ID {}: {err}", user.teacher_id);
            }
        }
    }

    if changed_tokens {
        save_config(&config)?;
    }

    println!("✅ All syncs completed!");
    Ok(())
}

fn parse_interval(interval_str: Option<&str>) -> Result<u64> {
    let Some(interval_str) = interval_str else {
        return Ok(3_600_000);
    };

    if interval_str.chars().all(|c| c.is_ascii_digit()) {
        return interval_str
            .parse::<u64>()
            .context("Invalid numeric interval");
    }

    let chars: Vec<char> = interval_str.chars().collect();
    let mut index = 0usize;
    let mut total_ms = 0u64;

    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            bail!(
                "Invalid interval format: {}. Use format like \"1h\", \"30m\", \"1h30m\", or milliseconds.",
                interval_str
            );
        }

        let start = index;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }

        if index >= chars.len() {
            bail!(
                "Invalid interval format: {}. Use format like \"1h\", \"30m\", \"1h30m\", or milliseconds.",
                interval_str
            );
        }

        let value: u64 = chars[start..index]
            .iter()
            .collect::<String>()
            .parse()
            .context("Invalid interval number")?;

        let unit = chars[index].to_ascii_lowercase();
        index += 1;

        let multiplier = match unit {
            'h' => 60 * 60 * 1000,
            'm' => 60 * 1000,
            's' => 1000,
            _ => {
                bail!(
                    "Invalid interval unit in {}. Use h, m, or s.",
                    interval_str
                )
            }
        };

        total_ms = total_ms
            .checked_add(value.saturating_mul(multiplier))
            .ok_or_else(|| anyhow!("Interval value is too large"))?;
    }

    if total_ms == 0 {
        bail!(
            "Invalid interval format: {}. Use format like \"1h\", \"30m\", \"1h30m\", or milliseconds.",
            interval_str
        );
    }

    Ok(total_ms)
}

fn format_interval(ms: u64) -> String {
    let hours = ms / (60 * 60 * 1000);
    let minutes = (ms % (60 * 60 * 1000)) / (60 * 1000);
    let seconds = (ms % (60 * 1000)) / 1000;

    let mut output = String::new();
    if hours > 0 {
        output.push_str(&format!("{hours}h"));
    }
    if minutes > 0 {
        output.push_str(&format!("{minutes}m"));
    }
    if seconds > 0 && hours == 0 {
        output.push_str(&format!("{seconds}s"));
    }

    if output.is_empty() {
        format!("{ms}ms")
    } else {
        output
    }
}

#[cfg(unix)]
async fn wait_or_stop(interval_ms: u64) -> bool {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).ok();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => true,
        _ = async {
            if let Some(sig) = sigterm.as_mut() {
                sig.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => true,
        _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => false,
    }
}

#[cfg(not(unix))]
async fn wait_or_stop(interval_ms: u64) -> bool {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => true,
        _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => false,
    }
}

async fn watch_command(interval: Option<String>) -> Result<()> {
    let interval_ms = parse_interval(interval.as_deref())?;
    println!(
        "Starting continuous sync mode at {} interval",
        format_interval(interval_ms)
    );

    if let Err(err) = sync_command().await {
        eprintln!("Error during sync: {err}");
    }

    loop {
        let should_stop = wait_or_stop(interval_ms).await;
        if should_stop {
            println!("\n\n⏹  Stopping continuous sync...");
            break;
        }

        if let Err(err) = sync_command().await {
            eprintln!("Error during sync: {err}");
        }
    }

    Ok(())
}