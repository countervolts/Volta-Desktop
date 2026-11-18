# Security policy

## Supported versions

Security fixes go into the latest stable release and the current nightly build (the newest successful build of `main`). Older releases are not patched, so please update first and check the problem still exists.

## Reporting a vulnerability

**Please don't open a public issue for a security problem.**

Report it privately through GitHub:
<https://github.com/countervolts/Volta-Desktop/security/advisories/new>

Include as much of this as you can:

- What the problem is and what an attacker could do with it
- Steps to reproduce, or a proof of concept
- Your Volta version (Settings → About → Copy version info) and operating system
- Whether it needs a particular setup, such as a malicious server or a crafted audio file

You'll get an acknowledgement as soon as I can manage, and I'll keep you updated while it's looked at. Volta is maintained by one person, so there are no guaranteed response times, but security reports come first. Once a fix is released I'm happy to credit you, unless you'd rather stay anonymous.

## What's in scope

- Handling of sign-in details (Navidrome / Subsonic credentials and tokens)
- Parsing of untrusted input: server responses, audio files, artwork, lyrics, and tags
- Update checks and anything that opens links or files
- Data Volta writes to disk (the settings database, caches, and log files)

## Out of scope

- Problems in a Navidrome or Subsonic server itself. Report those to the server's project.
- Issues that need an attacker to already control your computer or your account on it
- Vulnerabilities in the vendored gpui crates that also exist upstream. Report those upstream.

## Good to know

- Volta keeps everything on your device. The only network traffic is to the server you connect to, LRCLIB for lyrics when that setting is on, MusicBrainz and Apple Music / Spotify link lookups when you use those features, and GitHub for update checks.
- Volta writes log files to its data folder (Settings → Help → Open logs folder). They don't contain your password, but they do include your server address, so look through them before attaching them to a public issue.
