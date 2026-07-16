# session.link CLI (`slink`)

Capture LLM sessions locally, publish the ones worth sharing.

```bash
npx session.link dev -- python agent.py   # record its LLM calls (nothing leaves your machine)
npx session.link open                     # browse captures locally, publish from the page
npx session.link push                     # validate → secret-scan → confirm → shareable URL
npx session.link login                    # sign in (GitHub) so pushes are yours
```

Install the `slink` command globally if you prefer:

```bash
npm i -g session.link
slink dev -- node agent.js
```

Capture is ambient and local; publishing is deliberate. Published sessions are
immutable, content-addressed, and unlisted by default. Full docs:
<https://session.link>.
