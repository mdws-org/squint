# Mac App Store listing

The text App Store Connect shows for the `SquintStore` edition, kept here so it changes with the code it describes. The listing is written in US English, because the primary store language is English (U.S.).

## Name (30 characters at most)

Squint Image Optimizer

## Subtitle (30 characters at most)

Shrink photos, remove location

## Promotional text (170 characters at most)

Smaller photos that keep their color profile and HDR, with the location removed, from the window or the right-click menu in Finder.

## Description

Squint makes images smaller and removes the location, camera and date details stored in a photo. It keeps the color profile and the HDR that other tools discard, so an iPhone photo looks the same afterwards.

Drop files on the window, or select them in Finder and choose one of three entries from the Services menu:

Shrink re-encodes a JPEG once at a fixed quality and recompresses a PNG losslessly, then replaces the file. A 12-megapixel iPhone photo goes from 1.5 MB to about 0.7 MB with its HDR kept. A PDF has the pictures inside it re-encoded.

Shrink to a Quality Target measures each picture with SSIMULACRA2, a perceptual metric, and finds the smallest file that still meets the score you set. A flat screenshot and a grainy photograph each get the setting they need instead of one number for both.

Remove Location Data takes out the GPS position, the camera details and the timestamps without re-encoding, so the pixels stay exactly as they were. It works on JPEG, PNG, HEIC, AVIF, WebP, TIFF, GIF and PDF files.

Squint keeps the color profile (a Display P3 photo stays Display P3), the HDR gain map of an iPhone photo, Finder tags and the file's creation date. It works offline: there is no upload, account or analytics code in it.

This edition replaces files in place. Copies sized for email and social posts, conversion to AVIF and PNG color reduction are in the full edition, which is free and open source at github.com/mdws-org/squint.

## Keywords (100 characters at most, comma separated, no words already in the name or subtitle)

compress,jpeg,png,heic,pdf,exif,metadata,gps,privacy,reduce,file size,strip,hdr,color profile

## URLs

- Support: https://github.com/mdws-org/squint/issues
- Marketing: https://thebenmeadows.com/blog/squint/
- Privacy policy: https://github.com/mdws-org/squint/blob/main/PRIVACY.md

## Categories, price, rating

- Primary category: Photo & Video. Secondary category: Utilities.
- Price: free, in every territory.
- Age rating: 4+.
- Copyright: 2026 Benjamin Meadows.
- App Privacy: Data Not Collected.
- Release: manual, after approval.

## Notes for App Review

Squint changes the image files it is given. Drag one or more JPEG, PNG or HEIC files onto the window, or select them in Finder and choose Services, then "Squint: Shrink", "Squint: Shrink to a Quality Target" or "Squint: Remove Location Data". Each replaces the chosen file in place, which the App Sandbox user-selected read-write entitlement covers. Any photo exported from the Photos app works as a test file; a file in Downloads, Desktop or Documents raises the system's folder access request the first time.

The application does not open network connections. The link at the foot of its window opens the project's GitHub releases page in the browser, where a separately distributed edition of the same open source project (GPL-3.0, free) is published.
