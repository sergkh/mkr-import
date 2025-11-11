const fs = require('fs');
const path = require('path');
const readline = require('readline');
const { google } = require('googleapis');
const moment = require('moment');

// If modifying scopes, delete tokens
const SCOPES = ['https://www.googleapis.com/auth/calendar'];

const CONFIG_DIR = 'config';
const CONFIG_PATH = path.join(CONFIG_DIR, 'config.json');
const CREDENTIALS_PATH = path.join(CONFIG_DIR, 'credentials.json');

if (!fs.existsSync(CONFIG_DIR)) fs.mkdirSync(CONFIG_DIR, { recursive: true });

function loadConfig() {
  if (!fs.existsSync(CONFIG_PATH)) return { users: [], timeZone: "Europe/Kyiv" };
  return JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'));
}

function saveConfig(config) {
  fs.writeFileSync(CONFIG_PATH, JSON.stringify(config, null, 2));
}

// Question helper
function question(query) {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
  return new Promise(resolve => {
    rl.question(query, answer => {
      rl.close();
      resolve(answer);
    });
  });
}

// Load credentials
function loadCredentials() {
  if (!fs.existsSync(CREDENTIALS_PATH)) {
    throw new Error('credentials.json not found. Please add your Google OAuth credentials.');
  }
  return JSON.parse(fs.readFileSync(CREDENTIALS_PATH, 'utf8'));
}

// Create OAuth2 client
function createOAuth2Client(credentials) {
  const { client_secret, client_id, redirect_uris } = credentials.installed;
  return new google.auth.OAuth2(
    client_id, client_secret, redirect_uris[0]
  );
}

// Get access token interactively
async function getAccessToken(oAuth2Client) {
  const authUrl = oAuth2Client.generateAuthUrl({
    access_type: 'offline',
    scope: SCOPES,
  });
  
  console.log(`\nAuthorize this app by visiting this URL:\n${authUrl}\n`);
  const code = await question('Enter the code from that page here: ');
  
  return new Promise((resolve, reject) => {
    oAuth2Client.getToken(code, (err, token) => {
      if (err) {
        reject(err);
        return;
      }
      resolve(token);
    });
  });
}

async function authorize(credentials, userToken) {
  const oAuth2Client = createOAuth2Client(credentials);
  
  if (userToken) {
    oAuth2Client.setCredentials(userToken);
    return oAuth2Client;
  }
  
  const token = await getAccessToken(oAuth2Client);
  oAuth2Client.setCredentials(token);
  return { oAuth2Client, token };
}

async function createCommand() {
  try {
    const credentials = loadCredentials();
    const config = loadConfig();
    
    console.log('\n=== Creating New User ===\n');
    
    const calendarId = await question('Enter Calendar ID: ');
    if (!calendarId) {
      console.error('Calendar ID is required');
      process.exit(1);
    }
    
    const teacherIdStr = await question('Enter Teacher ID: ');
    const teacherId = parseInt(teacherIdStr, 10);
    if (isNaN(teacherId)) {
      console.error('Invalid Teacher ID');
      process.exit(1);
    }
    
    // Check if calendar ID already exists
    if (config.users.some(u => u.calendarId === calendarId)) {
      console.error(`Calendar ID ${calendarId} already exists`);
      process.exit(1);
    }
    
    console.log('\nGetting OAuth token...');
    const { token } = await authorize(credentials, null);
    
    // Add new user
    config.users.push({
      calendarId,
      teacherId,
      token
    });
    
    saveConfig(config);
    console.log(`\n✅ User created successfully!`);
    console.log(`   Calendar ID: ${calendarId}`);
    console.log(`   Teacher ID: ${teacherId}`);
  } catch (err) {
    console.error('Error creating user:', err.message);
    process.exit(1);
  }
}

// Remove command
async function removeCommand() {
  const config = loadConfig();
  
  if (config.users.length === 0) {
    console.log('No users configured.');
    return;
  }
  
  console.log('\n=== Remove User ===\n');
  console.log('Current users:');
  config.users.forEach((user, index) => {
    console.log(`  ${index + 1}. Calendar ID: ${user.calendarId}, Teacher ID: ${user.teacherId}`);
  });
  console.log('');
  
  const answer = await question('Enter the number of the user to remove (or "cancel"): ');
  
  if (answer.toLowerCase() === 'cancel') {
    console.log('Cancelled.');
    return;
  }
  
  const index = parseInt(answer, 10) - 1;
  if (isNaN(index) || index < 0 || index >= config.users.length) {
    console.error('Invalid selection');
    process.exit(1);
  }
  
  const removed = config.users.splice(index, 1)[0];
  saveConfig(config);
  console.log(`\n✅ Removed user: Calendar ID ${removed.calendarId}, Teacher ID ${removed.teacherId}`);
}

// Events equal check
function eventsEqual(existing, incoming) {
  return (
    existing.summary === incoming.summary &&
    existing.location === incoming.location &&
    moment(existing.start.dateTime).isSame(moment(incoming.start.dateTime)) &&
    moment(existing.end.dateTime).isSame(moment(incoming.end.dateTime))
  );
}

// Sync schedule events for a single user
async function syncScheduleEventsForUser(auth, user, timeZone) {
  const calendar = google.calendar({ version: "v3", auth });

  const startDate = moment().format("YYYY-MM-DD");
  const endDate = moment().add(2, 'weeks').format("YYYY-MM-DD");
  const url =
    `https://mkr.sergkh.com/structures/0/chairs/182/teachers/${user.teacherId}/schedule?startDate=${startDate}&endDate=${endDate}`;
  
  console.log(`\n📋 Syncing for Calendar ID: ${user.calendarId}, Teacher ID: ${user.teacherId}`);
  
  const response = await fetch(url);
  const events = await response.json();

  console.log(`Fetched ${events.length} events`);

  const grouped = {};  
  // pre create empty arrays for each date, to cleaning events for free days
  for(let date = startDate; date <= endDate; date = moment(date).add(1, 'days').format("YYYY-MM-DD")) {
    grouped[date] = [];
  }

  for (const e of events) {
    const date = moment(e.start, "YYYY-MM-DD HH:mm").format("YYYY-MM-DD");
    grouped[date].push(e);
  }

  for (const [date, dayEvents] of Object.entries(grouped)) {
    console.log(`\n📅 Syncing ${date} (${dayEvents.length} events)`);

    const newItems = dayEvents.map(item => {
      const start = moment(item.start, "YYYY-MM-DD HH:mm").toISOString();
      const end = moment(item.end, "YYYY-MM-DD HH:mm").toISOString();
      return {
        summary: `${item.name} (${item.group})`,
        location: item.place,
        description: `Type: ${item.type}`,
        start: { dateTime: start, timeZone: timeZone || "Europe/Kyiv" },
        end: { dateTime: end, timeZone: timeZone || "Europe/Kyiv" },
      };
    });

    // 1. Delete existing events for this date
    const dayStart = moment(date).startOf("day").toISOString();
    const dayEnd = moment(date).endOf("day").toISOString();

    const existing = await calendar.events.list({
      calendarId: user.calendarId,
      timeMin: dayStart,
      timeMax: dayEnd,
      singleEvents: true,
      orderBy: "startTime",
    });
    
    const existingItems = existing.data.items;
    const identical = existingItems.length === newItems.length && existingItems.every((ev, i) => eventsEqual(ev, newItems[i]));

    if (identical) {
      console.log(`✅ ${date} unchanged, skipping`);
      continue; // skip to next date
    } else {
      console.log(`🔄 ${date} has changes, updating...`);
    }

    if (existingItems.length > 0) {
      console.log(`🗑 Removing ${existingItems.length} old events`);
      for (const ev of existingItems) {
        try {
          await calendar.events.delete({ calendarId: user.calendarId, eventId: ev.id});
        } catch (err) {
          console.error("❌ Error deleting event:", err.message);
        }
      }
    }

    for (const event of newItems) {
      try {
        const res = await calendar.events.insert({ calendarId: user.calendarId, resource: event });
        console.log("✅ Created:", event.summary, "->", res.data.htmlLink);
      } catch (err) {
        console.error("❌ Error creating event:", err.message);
      }
    }
  }
}

async function syncCommand() {
  try {
    const credentials = loadCredentials();
    const config = loadConfig();
    
    if (config.users.length === 0) {
      console.log('No users configured. Use "create" command to add a user.');
      process.exit(1);
    }
    
    console.log(`Syncing ${config.users.length} user(s)...`);
    
    for (const user of config.users) {
      try {
        const auth = await authorize(credentials, user.token);
        await syncScheduleEventsForUser(auth, user, config.timeZone);
        console.log(`\n✅ Completed sync for Teacher ID: ${user.teacherId}\n`);
      } catch (err) {
        console.error(`Error syncing Teacher ID ${user.teacherId}:`, err.message);
      }
    }
    
    console.log('✅ All syncs completed!');
  } catch (err) {
    console.error('Error during sync:', err.message);
    process.exit(1);
  }
}

// Parse interval string to milliseconds
// Supports: "1h", "30m", "1h30m", "3600000" (ms), etc.
function parseInterval(intervalStr) {
  if (!intervalStr) return 3600000; // Default 1 hour
  
  // If it's just a number, treat as milliseconds
  const numOnly = parseInt(intervalStr, 10);
  if (!isNaN(numOnly) && intervalStr === numOnly.toString()) {
    return numOnly;
  }
  
  // Parse time units
  let totalMs = 0;
  const regex = /(\d+)([hms])/gi;
  let match;
  
  while ((match = regex.exec(intervalStr)) !== null) {
    const value = parseInt(match[1], 10);
    const unit = match[2].toLowerCase();
    
    switch (unit) {
      case 'h':
        totalMs += value * 60 * 60 * 1000;
        break;
      case 'm':
        totalMs += value * 60 * 1000;
        break;
      case 's':
        totalMs += value * 1000;
        break;
    }
  }
  
  if (totalMs === 0) {
    throw new Error(`Invalid interval format: ${intervalStr}. Use format like "1h", "30m", "1h30m", or milliseconds.`);
  }
  
  return totalMs;
}

// Format milliseconds to human-readable string
function formatInterval(ms) {
  const hours = Math.floor(ms / (60 * 60 * 1000));
  const minutes = Math.floor((ms % (60 * 60 * 1000)) / (60 * 1000));
  const seconds = Math.floor((ms % (60 * 1000)) / 1000);
  
  const parts = [];
  if (hours > 0) parts.push(`${hours}h`);
  if (minutes > 0) parts.push(`${minutes}m`);
  if (seconds > 0 && hours === 0) parts.push(`${seconds}s`);
  
  return parts.join('') || `${ms}ms`;
}

// Watch command - sync continuously
async function watchCommand() {
  let intervalMs = 3600000; // Default 1 hour
  let shouldStop = false;
  
  const intervalIndex = process.argv.indexOf('--interval');
  if (intervalIndex !== -1 && process.argv[intervalIndex + 1]) {
    try {
      intervalMs = parseInterval(process.argv[intervalIndex + 1]);
    } catch (err) {
      console.error(err.message);
      process.exit(1);
    }
  }
  
  console.log(`Starting continuous sync mode at ${formatInterval(intervalMs)} interval`);
  
  process.on('SIGINT', () => {
    console.log('\n\n⏹  Stopping continuous sync...');
    shouldStop = true;
  });
  
  process.on('SIGTERM', () => {
    console.log('\n\n⏹  Stopping continuous sync...');
    shouldStop = true;
  });
  
  await syncCommand();

  while (!shouldStop) {
    await new Promise(resolve => {
      let timeout;
      let checkInterval;
      
      timeout = setTimeout(() => {
        clearInterval(checkInterval);
        resolve();
      }, intervalMs);
      
      // Check if we should stop periodically
      checkInterval = setInterval(() => {
        if (shouldStop) {
          clearTimeout(timeout);
          clearInterval(checkInterval);
          resolve();
        }
      }, 1000);
    });
    
    if (!shouldStop) await syncCommand();
  }
}

// Main entry point
const command = process.argv[2] || 'sync';

switch (command) {
  case 'create':
    return createCommand();
  case 'remove':
    return removeCommand();
  case 'sync':
    return syncCommand();
  case 'watch':
    return watchCommand();
  default:
    console.log('Usage: node index.js [create|remove|sync|watch]');
    console.log('  create - Add a new user');
    console.log('  remove - Remove a user');
    console.log('  sync   - Sync all users once (default)');
    console.log('  watch  - Sync continuously at intervals');
    console.log('');
    console.log('Watch options:');
    console.log('  --interval <time>  Sync interval (default: 1h)');
    console.log('                     Examples: "1h", "30m", "1h30m", "3600000" (ms)');
    process.exit(1);
}
