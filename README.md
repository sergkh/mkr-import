# MKR syncing with Google Calendar

This CLI syncs the MKR schedule with Google Calendar.

## Build

```bash
cargo build --release
```

## Usage

First one need to register the app on the [Google cloud console](https://console.cloud.google.com/) and download `credentials.json` for the app. 
The application type should be Desktop

Create new entry:

```bash
$ ./mkr-import [create|remove|sync|watch]
```

Watch mode supports custom intervals:

```bash
$ ./mkr-import watch --interval 30m
$ ./mkr-import watch --interval 1h30m
$ ./mkr-import watch --interval 3600000
```

## Configuration

Configuration is stored in `config/config.json`.

Default config shape:

```json
{
	"users": [],
	"timeZone": "Europe/Kyiv"
}
```