//! Timers, animation frames, queueMicrotask, navigator and matchMedia as seen from
//! script, driven by the browsing context's event loop.

use std::time::{Duration, Instant};

use axiom_engine::BrowsingContext;

fn page(html: &str) -> BrowsingContext {
    let mut ctx = BrowsingContext::new(400, 300);
    ctx.load_local_html("about:timers", html, false)
        .expect("load html");
    ctx
}

fn eval(ctx: &mut BrowsingContext, src: &str) -> String {
    let js = ctx.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

/// Ticks until `cond` evaluates to "true" (or panics after a second).
fn tick_until(ctx: &mut BrowsingContext, cond: &str) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while eval(ctx, cond) != "true" {
        assert!(Instant::now() < deadline, "timed out waiting for {cond}");
        ctx.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn timeouts_run_in_order_with_arguments_and_can_be_cleared() {
    let mut ctx = page(
        "<script>
          var log = [];
          setTimeout(function (a, b) { log.push('args' + a + b); }, 0, 1, 2);
          setTimeout(\"log.push('string')\", 0);
          var cleared = setTimeout(function () { log.push('cleared'); }, 0);
          clearTimeout(cleared);
          setTimeout(function () { log.push('later'); }, 100);
          setTimeout(function () {
            log.push('outer');
            setTimeout(function () { log.push('nested'); }, 0);
          }, 1);
        </script>",
    );
    assert!(ctx.run_until_idle(Duration::from_secs(2)));
    assert_eq!(
        eval(&mut ctx, "log.join()"),
        "args12,string,outer,nested,later"
    );
    assert!(ctx.timers.is_empty());
}

#[test]
fn intervals_repeat_until_cleared_without_blocking_idle() {
    let mut ctx = page(
        "<script>
          var n = 0;
          var forever = setInterval(function () {}, 10);
          var iv = setInterval(function () { if (++n === 3) clearInterval(iv); }, 0);
        </script>",
    );
    tick_until(&mut ctx, "n === 3");
    ctx.tick();
    assert_eq!(ctx.timers.len(), 1, "only the endless interval remains");
    assert!(ctx.run_until_idle(Duration::from_millis(200)));
    assert_eq!(eval(&mut ctx, "clearTimeout(forever); n"), "3");
    ctx.tick();
    assert!(ctx.timers.is_empty(), "clearTimeout also clears intervals");
}

#[test]
fn uncaught_timer_exceptions_reach_onerror_and_diagnostics() {
    let mut ctx = page(
        "<script>
          window.onerror = function (message) { window.seen = message; };
          setTimeout(function () { throw new Error('boom'); }, 0);
          setTimeout(function () { window.after = true; }, 0);
        </script>",
    );
    assert!(ctx.run_until_idle(Duration::from_secs(1)));
    assert_eq!(eval(&mut ctx, "window.seen"), "Uncaught Error: boom");
    assert_eq!(eval(&mut ctx, "window.after"), "true");
    let errors = ctx.script_errors();
    assert!(
        errors
            .iter()
            .any(|e| e.label == "timer callback" && e.message.contains("boom")),
        "{errors:?}"
    );
}

#[test]
fn animation_frames_run_once_per_request_and_can_be_cancelled() {
    let mut ctx = page(
        "<script>
          var frames = [];
          requestAnimationFrame(function (t) {
            frames.push(typeof t);
            requestAnimationFrame(function () { frames.push('second'); });
          });
          var dropped = requestAnimationFrame(function () { frames.push('cancelled'); });
          cancelAnimationFrame(dropped);
        </script>",
    );
    assert!(ctx.run_until_idle(Duration::from_secs(1)));
    assert_eq!(eval(&mut ctx, "frames.join()"), "number,second");
}

#[test]
fn queue_microtask_runs_after_the_current_script() {
    let mut ctx = page("<script>var order = [];</script>");
    assert_eq!(
        eval(
            &mut ctx,
            "queueMicrotask(function () { order.push('micro'); }); order.push('sync'); 0"
        ),
        "0"
    );
    assert_eq!(eval(&mut ctx, "order.join()"), "sync,micro");
    assert_eq!(
        eval(
            &mut ctx,
            "try { queueMicrotask(1); 'no' } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn navigator_describes_axiom_honestly() {
    let mut ctx = page("<p>");
    assert_eq!(
        eval(&mut ctx, "navigator.userAgent"),
        axiom_net::AXIOM_USER_AGENT
    );
    assert_eq!(
        eval(
            &mut ctx,
            "/Chrome|Safari|Firefox/.test(navigator.userAgent)"
        ),
        "false"
    );
    assert_eq!(eval(&mut ctx, "navigator.language"), "en-US");
    assert_eq!(
        eval(&mut ctx, "JSON.stringify(navigator.languages)"),
        r#"["en-US","en"]"#
    );
    assert_eq!(
        eval(
            &mut ctx,
            "Object.isFrozen(navigator.languages) && navigator.languages === navigator.languages"
        ),
        "true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "navigator instanceof Navigator && window.clientInformation === navigator"
        ),
        "true"
    );
    assert_eq!(
        eval(&mut ctx, "navigator.onLine && !navigator.webdriver"),
        "true"
    );
    assert_eq!(eval(&mut ctx, "navigator.hardwareConcurrency >= 1"), "true");
    assert_eq!(eval(&mut ctx, "'serviceWorker' in navigator"), "false");
}

#[test]
fn viewport_metrics_and_active_element() {
    let mut ctx = page("<p>");
    assert_eq!(
        eval(&mut ctx, "innerWidth + ' ' + devicePixelRatio"),
        "400 1"
    );
    assert_eq!(
        eval(&mut ctx, "innerHeight > 0 && innerHeight <= 300"),
        "true"
    );
    ctx.set_viewport(640, 300);
    ctx.tick();
    assert_eq!(eval(&mut ctx, "innerWidth"), "640");
    assert_eq!(eval(&mut ctx, "innerWidth = 5; innerWidth"), "5");
    assert_eq!(
        eval(&mut ctx, "document.activeElement === document.body"),
        "true"
    );
}

#[test]
fn match_media_follows_the_viewport_and_fires_change() {
    let mut ctx = page("<p>");
    assert_eq!(
        eval(&mut ctx, "matchMedia('(min-width: 300px)').matches"),
        "true"
    );
    assert_eq!(
        eval(&mut ctx, "matchMedia('(min-width: 500px)').matches"),
        "false"
    );
    assert_eq!(
        eval(&mut ctx, "matchMedia('  screen  and (color) ').media"),
        "screen and (color)"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var mq = matchMedia('(min-width: 500px)'); var changes = []; \
             mq.addEventListener('change', function (e) { changes.push(e.matches + ' ' + e.media); }); \
             mq.addListener(function (e) { changes.push('legacy ' + (e instanceof MediaQueryListEvent)); }); \
             mq instanceof MediaQueryList && mq instanceof EventTarget"
        ),
        "true"
    );
    ctx.set_viewport(600, 300);
    ctx.tick();
    assert_eq!(
        eval(&mut ctx, "changes.join()"),
        "true (min-width: 500px),legacy true"
    );
    ctx.tick();
    assert_eq!(eval(&mut ctx, "changes.length"), "2", "no change, no event");
}
