import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for the BB-194 Agents page UX findings:
//
//   BB-198  every card menu (⋮) sits inside its card and is hit-testable
//   BB-201  the agent panel header fits 390 and Kill stays visible
//   BB-202  the fleet roll-up is not clipped at 1280 or 390
//   BB-209  tags, budget chips, footer text, legend and stat labels clear AA
//   BB-211  Clear orchestrator works from the card menu, and the top-level
//           drop zone stops advertising drag on touch
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-ux.browser.mjs
//
// The fixture creates its own project and agents, so it is safe on a scratch
// database and does not depend on the production board.

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

const WIDTHS = [1680, 1280, 390];
const AA_NORMAL = 4.5;

try {
  const context = await browser.newContext({ viewport: { width: 1680, height: 1000 } });
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();
  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const fixture = await page.evaluate(
    async ({ stamp }) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const project = await api("POST", "/api/projects", {
        key: `U${stamp}`,
        name: `Agents UX ${stamp}`,
      });
      const issue = (title) => api("POST", "/api/issues", { title, project: project.key });

      // A production-shaped roster: one orchestrator with a deep tree, a few
      // satellites and enough independent agents to fill the grid, with mixed
      // claim states so the server roll-up is long enough to clip if the header
      // still gives it a sliver of width.
      const root = {
        name: `ux-root-${stamp}`,
        session: `ses_ux_root_${stamp}`,
      };
      const children = [];
      for (let i = 0; i < 22; i++) {
        children.push({
          name: `ux-child-${String(i).padStart(2, "0")}-${stamp}`,
          session: `ses_ux_child_${i}_${stamp}`,
        });
      }
      const satellites = [];
      for (let i = 0; i < 16; i++) {
        satellites.push({
          name: `ux-solo-${String(i).padStart(2, "0")}-${stamp}`,
          session: `ses_ux_solo_${i}_${stamp}`,
        });
      }

      const register = (agent, sessionId, parentSessionId) =>
        api("POST", "/api/agents/register", {
          agent_name: agent,
          session_id: sessionId,
          ...(parentSessionId ? { parent_session_id: parentSessionId } : {}),
          host: "test",
          pid: 42,
        });

      const states = ["implementing", "awaiting-review", "verifying", "blocked", "awaiting-ci"];

      // `count` exclusive claims in distinct states.
      const claim = async (agent, sessionId, count, offset) => {
        for (let i = 0; i < count; i++) {
          const created = await issue(`${agent} ${stamp} ${i + 1}`);
          await api("POST", `/api/issues/${created.key}/claim`, {
            agent_name: agent,
            session_id: sessionId,
            message: `${agent} fixture`,
            state: states[(offset + i) % states.length],
          });
        }
      };

      await register(root.name, root.session);
      await claim(root.name, root.session, 3, 0);
      let n = 0;
      for (const child of children) {
        await register(child.name, child.session, root.session);
        await claim(child.name, child.session, 1 + (n % 3), n);
        n++;
      }
      for (const solo of satellites) {
        await register(solo.name, solo.session);
        await claim(solo.name, solo.session, 1 + (n % 2), n);
        n++;
      }

      return {
        project: project.key,
        root,
        child: children[0],
        childSession: children[0].session,
        solo: satellites[0],
        rosterSize: 1 + children.length + satellites.length,
      };
    },
    { stamp },
  );

  // --- helpers -------------------------------------------------------------

  const rosterRow = async (name) => {
    const response = await context.request.get(`${base}/api/agents/roster`);
    assert.ok(response.ok(), "roster request succeeds");
    const roster = await response.json();
    const row = roster.find((entry) => entry.agent_name === name);
    assert.ok(row, `${name} is on the roster`);
    return row;
  };

  const openAgents = async () => {
    if (await page.locator(".agent-card").count()) return;
    await page.getByRole("button", { name: "Agents", exact: true }).click();
    await page.waitForSelector(".agent-card");
    await page.waitForTimeout(250);
  };

  // Effective, composited contrast for one element: walk the ancestor chain,
  // composite every background over the page background, then compare the
  // computed text colour against it with the WCAG 2.x relative luminance.
  const contrastOf = (selector) =>
    page.evaluate((selector) => {
      const el = document.querySelector(selector);
      if (!el) return null;
      const parse = (value) => {
        const rgb = value.match(/^rgba?\(([^)]+)\)/);
        if (rgb) {
          const parts = rgb[1].split(/[ ,/]+/).filter(Boolean).map(Number);
          return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
        }
        const srgb = value.match(/^color\(srgb\s+([^)]+)\)/);
        if (srgb) {
          const parts = srgb[1].split(/[ /]+/).filter(Boolean).map(Number);
          return { r: parts[0] * 255, g: parts[1] * 255, b: parts[2] * 255, a: parts.length > 3 ? parts[3] : 1 };
        }
        return null;
      };
      const over = (fg, bg) => ({
        r: fg.r * fg.a + bg.r * (1 - fg.a),
        g: fg.g * fg.a + bg.g * (1 - fg.a),
        b: fg.b * fg.a + bg.b * (1 - fg.a),
        a: 1,
      });
      const chain = [];
      for (let node = el; node && node.nodeType === 1; node = node.parentElement) chain.push(node);
      let bg = { r: 255, g: 255, b: 255, a: 1 };
      for (const node of chain.reverse()) {
        const paint = parse(getComputedStyle(node).backgroundColor);
        if (paint && paint.a > 0) bg = over(paint, bg);
      }
      const fg = parse(getComputedStyle(el).color);
      const text = over(fg, bg);
      const lum = (c) => {
        const ch = (v) => {
          const s = v / 255;
          return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
        };
        return 0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b);
      };
      const a = lum(text);
      const b = lum(bg);
      const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      return {
        ratio: Math.round(ratio * 100) / 100,
        fontSize: getComputedStyle(el).fontSize,
        text: (el.textContent ?? "").trim().slice(0, 40),
      };
    }, selector);

  const cardGeometry = () =>
    page.evaluate(() => {
      const out = [];
      for (const card of document.querySelectorAll(".agent-card")) {
        const cardRect = card.getBoundingClientRect();
        const footer = card.querySelector(".afoot");
        const footRect = footer.getBoundingClientRect();
        const btn = card.querySelector(".card-menu-btn");
        const btnRect = btn.getBoundingClientRect();
        out.push({
          name: card.getAttribute("data-agent"),
          cardRight: cardRect.right,
          cardLeft: cardRect.left,
          cardTop: cardRect.top,
          cardBottom: cardRect.bottom,
          footerRight: footRect.right,
          footerBottom: footRect.bottom,
          btnLeft: btnRect.left,
          btnRight: btnRect.right,
          btnWidth: btnRect.width,
          btnHeight: btnRect.height,
          footerWraps: footRect.height > btnRect.height + 6,
        });
      }
      return out;
    });

  const hitTestCardMenu = async (index) => {
    const card = page.locator(".agent-card").nth(index);
    await card.scrollIntoViewIfNeeded();
    await page.waitForTimeout(30);
    return card.evaluate((el) => {
      const btn = el.querySelector(".card-menu-btn");
      const r = btn.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height / 2;
      const hit = document.elementFromPoint(cx, cy);
      return {
        name: el.getAttribute("data-agent"),
        inViewport: r.top >= 0 && r.left >= 0 && r.bottom <= window.innerHeight && r.right <= window.innerWidth,
        hitIsButton: !!hit && (hit === btn || btn.contains(hit)),
        hitTag: hit ? `${hit.tagName}.${hit.className}`.slice(0, 60) : "none",
      };
    });
  };

  // --- per-width checks ----------------------------------------------------

  for (const width of WIDTHS) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.reload();
    await openAgents();
    await page.waitForTimeout(200);

    const count = await page.locator(".agent-card").count();
    assert.ok(count >= fixture.rosterSize, `${width}: the fixture roster renders (${count} cards)`);

    // BB-198: footer and menu containment.
    const geometry = await cardGeometry();
    for (const g of geometry) {
      assert.ok(
        g.footerRight <= g.cardRight + 0.5,
        `${width}: ${g.name} footer ends inside the card (${g.footerRight.toFixed(1)} <= ${g.cardRight.toFixed(1)})`,
      );
      assert.ok(
        g.btnRight <= g.cardRight + 0.5,
        `${width}: ${g.name} menu stays inside the card (${g.btnRight.toFixed(1)} <= ${g.cardRight.toFixed(1)})`,
      );
      assert.ok(
        g.btnLeft >= g.cardLeft - 0.5,
        `${width}: ${g.name} menu does not hang off the left edge`,
      );
      assert.ok(g.btnWidth > 16 && g.btnHeight > 16, `${width}: ${g.name} menu button has a real hit area`);
    }

    // ... and every one of them is actually clickable at the card edge.
    for (let i = 0; i < count; i++) {
      const hit = await hitTestCardMenu(i);
      assert.ok(hit.inViewport, `${width}: ${hit.name} menu button is on screen after scrolling`);
      assert.ok(
        hit.hitIsButton,
        `${width}: ${hit.name} menu is hit-testable (elementFromPoint hit ${hit.hitTag})`,
      );
    }

    // BB-202: the roll-up is fully rendered, not clipped to a sliver.
    const summary = await page.evaluate(() => {
      const widget = document.querySelector(".widget-line");
      const progress = document.querySelector(".progress");
      const stats = document.querySelector(".stats");
      const grid = document.querySelector(".summary");
      const wr = widget.getBoundingClientRect();
      return {
        text: widget.textContent.trim(),
        scrollWidth: widget.scrollWidth,
        clientWidth: widget.clientWidth,
        scrollHeight: widget.scrollHeight,
        clientHeight: widget.clientHeight,
        progressWidth: progress.getBoundingClientRect().width,
        statsWidth: stats.getBoundingClientRect().width,
        gridRight: grid.getBoundingClientRect().right,
        gridScrollWidth: grid.scrollWidth,
        gridClientWidth: grid.clientWidth,
        textRight: wr.right,
        docScrollWidth: document.documentElement.scrollWidth,
        viewport: window.innerWidth,
      };
    });
    const status = await context.request.get(`${base}/api/agents/status`);
    const line = (await status.json()).line;
    assert.ok(line.length > 60, `${width}: fixture roll-up is long enough to clip (${line.length} chars)`);
    assert.equal(summary.text, line, `${width}: the summary shows the whole server roll-up`);
    assert.ok(
      summary.scrollWidth <= summary.clientWidth + 1,
      `${width}: the roll-up wraps instead of clipping (${summary.scrollWidth} <= ${summary.clientWidth})`,
    );
    assert.ok(
      summary.scrollHeight <= summary.clientHeight + 1,
      `${width}: the roll-up is not cut off vertically`,
    );
    if (width >= 1280) {
      assert.ok(
        summary.progressWidth >= 300,
        `${width}: the progress block gets real width (${summary.progressWidth.toFixed(0)}px)`,
      );
    }
    assert.ok(
      summary.gridScrollWidth <= summary.gridClientWidth + 1,
      `${width}: the summary card does not overflow horizontally`,
    );
    assert.ok(
      summary.docScrollWidth <= summary.viewport + 1,
      `${width}: the page does not scroll horizontally (${summary.docScrollWidth} <= ${summary.viewport})`,
    );

    // BB-209: functional small text clears AA on the composited background.
    for (const selector of [
      ".role-badge.sub",
      ".budget:not(.tree)",
      ".budget.tree",
      ".counts",
      ".gtick",
      ".stat span",
      ".lg",
      ".live-note",
    ]) {
      const measured = await contrastOf(selector);
      assert.ok(measured, `${width}: ${selector} is rendered`);
      assert.ok(
        measured.ratio >= AA_NORMAL,
        `${width}: ${selector} "${measured.text}" scores ${measured.ratio}:1 at ${measured.fontSize}, below AA ${AA_NORMAL}:1`,
      );
    }

    // BB-201: the agent panel fits and keeps its destructive action.
    await page.locator(`[data-agent="${fixture.root.name}"]`).click();
    await page.waitForSelector(".modal");
    await page.waitForTimeout(200);
    const panel = await page.evaluate(() => {
      const modal = document.querySelector(".modal");
      const head = document.querySelector(".mhead");
      const kill = [...document.querySelectorAll(".mhead-actions .btn")].find(
        (b) => b.textContent.trim() === "Kill",
      );
      const close = document.querySelector(".modal .close");
      const mr = modal.getBoundingClientRect();
      const hr = head.getBoundingClientRect();
      const kr = kill.getBoundingClientRect();
      const cr = close.getBoundingClientRect();
      const hit = document.elementFromPoint(kr.left + kr.width / 2, kr.top + kr.height / 2);
      return {
        modalLeft: mr.left,
        modalRight: mr.right,
        modalWidth: mr.width,
        modalScrollWidth: modal.scrollWidth,
        modalClientWidth: modal.clientWidth,
        headHeight: hr.height,
        killRight: kr.right,
        killBottom: kr.bottom,
        killHit: hit === kill || kill.contains(hit),
        closeRight: cr.right,
        viewport: window.innerWidth,
        docScrollWidth: document.documentElement.scrollWidth,
      };
    });
    assert.ok(panel.modalRight <= panel.viewport + 0.5, `${width}: the panel stays inside the viewport`);
    assert.ok(
      panel.modalScrollWidth <= panel.modalClientWidth + 1,
      `${width}: the panel content fits its width (${panel.modalScrollWidth} <= ${panel.modalClientWidth})`,
    );
    assert.ok(panel.headHeight <= 150, `${width}: the panel header reflows instead of stacking (${panel.headHeight.toFixed(0)}px)`);
    assert.ok(panel.killRight <= panel.modalRight + 0.5, `${width}: Kill stays inside the panel`);
    assert.ok(
      panel.killBottom <= (width === 390 ? 844 : 1000) + 0.5 && panel.killRight <= panel.viewport + 0.5,
      `${width}: Kill is on screen`,
    );
    assert.ok(panel.killHit, `${width}: Kill is hit-testable`);
    assert.ok(panel.closeRight <= panel.modalRight + 0.5, `${width}: Close stays inside the panel`);
    assert.ok(
      panel.docScrollWidth <= panel.viewport + 1,
      `${width}: the panel does not widen the page`,
    );
    await page.keyboard.press("Escape");
    await page.waitForSelector(".modal", { state: "detached", timeout: 3000 });
    assert.equal(await page.locator(".modal").count(), 0, `${width}: Escape closes the panel`);

    // BB-211: clearing a parent is a menu action, not a page-top drop.
    const card = page.locator(`[data-agent="${fixture.child.name}"]`);
    await card.scrollIntoViewIfNeeded();
    await card.locator(".card-menu-btn").click();
    await page.waitForSelector(".card-menu");
    const menuRect = await page.locator(".card-menu").boundingBox();
    assert.ok(menuRect, `${width}: the card menu opens`);
    assert.ok(
      menuRect.x >= -0.5 && menuRect.x + menuRect.width <= width + 0.5,
      `${width}: the card menu is inside the viewport (${menuRect.x.toFixed(0)} + ${menuRect.width.toFixed(0)})`,
    );
    await page.getByRole("menuitem", { name: "Clear orchestrator" }).click();
    await page.waitForTimeout(500);
    assert.ok(
      (await rosterRow(fixture.child.name)).parent_session_id == null,
      `${width}: Clear orchestrator removes the parent on the server`,
    );
    assert.equal(
      await card.getAttribute("data-parent"),
      "",
      `${width}: the card returns to the top level`,
    );

    // Restore the tree for the next width.
    await page.evaluate(
      async ({ name, parent }) => {
        const response = await fetch(`/api/agents/${name}/orchestrator`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ orchestrator: parent }),
        });
        if (!response.ok) throw new Error(`reparent: ${response.status}`);
      },
      { name: fixture.child.name, parent: fixture.root.name },
    );
  }

  // BB-211: on a touch device the drop zone stops advertising a drag that
  // cannot happen and points at the menu instead.
  const touchContext = await browser.newContext({
    viewport: { width: 390, height: 844 },
    hasTouch: true,
    isMobile: true,
  });
  await touchContext.addCookies([{ name: "bb_session", value: session, url: base }]);
  const touchPage = await touchContext.newPage();
  await touchPage.goto(base);
  if (!(await touchPage.locator(".agent-card").count())) {
    await touchPage.getByRole("button", { name: "Agents", exact: true }).click();
    await touchPage.waitForSelector(".agent-card");
  }
  const hints = await touchPage.evaluate(() => {
    const display = (selector) => {
      const el = document.querySelector(selector);
      return el ? getComputedStyle(el).display : "missing";
    };
    return { drop: display(".drop-hint"), touch: display(".touch-hint") };
  });
  assert.equal(hints.touch, "block", "touch: the drop zone explains the menu path");
  assert.equal(hints.drop, "none", "touch: the dead drag instruction is hidden");
  await touchContext.close();

  console.log("agents page UX browser regressions: passed", fixture.project);
} finally {
  await browser.close();
}
