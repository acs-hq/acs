# Scoop

`acs.json.in` is the template of the Scoop manifest. Each release fills it in
(`render-manifest.sh`, run by `.github/workflows/scoop-manifest.yml`) and ships the result as the
release file `acs.json`, so no separate bucket repository is needed:

```powershell
scoop install https://github.com/acs-hq/acs/releases/latest/download/acs.json
```

`scoop update acs` reads the same URL again. If ACS later gets a bucket, its manifest needs
`checkver` and `autoupdate` as well.
