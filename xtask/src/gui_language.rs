use super::gui::{Driver, Result};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub fn verify(driver: &Driver, output: &Path) -> Result<()> {
    let original = driver.script(
        r#"
        const select = document.querySelector('#language');
        return {language: select.value, options: select.options.length};
        "#,
    )?;
    let result = (|| -> Result<Value> {
        let mut receipts = Vec::new();
        driver.select("#language", "fa")?;
        query(driver, "chinese vertical")?;
        let query_state = state(driver)?;
        assert_selection(&query_state, "fa", &original)?;
        if query_state["role"] != "combobox"
            || query_state["autocomplete"] != "list"
            || query_state["expanded"] != "true"
            || query_state["controlledRole"] != "listbox"
            || query_state["matches"].as_array().map(Vec::len) != Some(2)
            || query_state["matches"].as_array().is_none_or(|options| {
                options.iter().any(|option| option["tabIndex"] != -1)
                    || options
                        .iter()
                        .filter(|option| option["selected"] == "true")
                        .count()
                        != 1
            })
            || query_state["activeSelected"] != "true"
            || query_state["focused"] != true
        {
            return Err(format!("Language search semantics failed: {query_state}").into());
        }
        receipts.push(checkpoint(driver, "filtered-query", "fa")?);

        query(driver, "a")?;
        let before_scroll = driver.script("return {page:scrollY,inspector:document.querySelector('#extract-inspector').scrollTop};")?;
        for _ in 0..8 {
            key(driver, "ArrowDown", false)?;
        }
        driver.wait("const input=document.querySelector('#language-search'),list=document.querySelector('#language-results'),active=document.getElementById(input?.getAttribute('aria-activedescendant'));if(!list||!active)return false;const bounds=list.getBoundingClientRect(),item=active.getBoundingClientRect(),top=bounds.top+list.clientTop;return list.scrollTop>0 && item.top>=top-1 && item.bottom<=top+list.clientHeight+1;", 5)?;
        let scrolled = state(driver)?;
        assert_selection(&scrolled, "fa", &original)?;
        let after_scroll = driver.script("const input=document.querySelector('#language-search'),active=document.getElementById(input.getAttribute('aria-activedescendant')),other=document.querySelector('#language-results [role=option]:not(.active)');return {page:scrollY,inspector:document.querySelector('#extract-inspector').scrollTop,activeBackground:getComputedStyle(active).backgroundColor,otherBackground:getComputedStyle(other).backgroundColor};")?;
        if before_scroll["page"] != after_scroll["page"]
            || before_scroll["inspector"] != after_scroll["inspector"]
            || after_scroll["activeBackground"] == after_scroll["otherBackground"]
        {
            return Err(format!("Keyboard selection scrolled outside the listbox or lost its highlight: {before_scroll} {after_scroll}").into());
        }

        query(driver, "zzzz-no-language")?;
        key(driver, "Enter", false)?;
        let no_match = state(driver)?;
        assert_selection(&no_match, "fa", &original)?;
        if no_match["query"] != "zzzz-no-language"
            || no_match["active"].as_str().is_some_and(|id| !id.is_empty())
            || no_match["matches"].as_array().map(Vec::len) != Some(0)
            || no_match["emptyStatus"] != true
        {
            return Err(format!("Unmatched language search changed selection: {no_match}").into());
        }
        receipts.push(checkpoint(driver, "no-match", "fa")?);
        driver.click("button[aria-label='Clear language search']")?;
        driver.wait(
            "return document.querySelector('#language-search')?.value === '';",
            5,
        )?;
        let cleared = state(driver)?;
        assert_selection(&cleared, "fa", &original)?;

        query(driver, "japanese")?;
        key(driver, "Enter", true)?;
        let composition = state(driver)?;
        assert_selection(&composition, "fa", &original)?;
        if composition["query"] != "japanese" {
            return Err(format!("Composing Enter committed a language: {composition}").into());
        }
        key(driver, "Escape", false)?;
        query(driver, "chinese vertical")?;
        key(driver, "ArrowDown", false)?;
        let moved = state(driver)?;
        assert_selection(&moved, "fa", &original)?;
        if moved["active"] != moved["matches"][1]["id"]
            || moved["activeSelected"] != "true"
            || moved["focused"] != true
        {
            return Err(format!("ArrowDown did not announce the next option: {moved}").into());
        }
        key(driver, "Escape", false)?;
        let cancelled = state(driver)?;
        assert_selection(&cancelled, "fa", &original)?;
        if cancelled["query"] != "" || cancelled["expanded"] != "false" {
            return Err(format!("Escape did not cancel language search: {cancelled}").into());
        }

        query(driver, "chinese vertical")?;
        key(driver, "ArrowDown", false)?;
        key(driver, "Enter", false)?;
        driver.wait("return document.querySelector('#language')?.value === 'chi_tra_vert' && document.querySelector('#language-search')?.value === '';", 5)?;
        let committed = state(driver)?;
        assert_selection(&committed, "chi_tra_vert", &original)?;
        receipts.push(checkpoint(
            driver,
            "explicit-keyboard-choice",
            "chi_tra_vert",
        )?);

        query(driver, "فارسی")?;
        driver.click("#language-option-fa")?;
        driver.wait("return document.querySelector('#language')?.value === 'fa' && document.querySelector('#language-search')?.value === '';", 5)?;
        let pointer_choice = state(driver)?;
        assert_selection(&pointer_choice, "fa", &original)?;
        if pointer_choice["focused"] != true {
            return Err(
                format!("Pointer choice did not return focus to search: {pointer_choice}").into(),
            );
        }
        query(driver, "japanese")?;
        key(driver, "Tab", false)?;
        let tab = state(driver)?;
        assert_selection(&tab, "fa", &original)?;
        if tab["expanded"] != "false" || tab["query"] != "japanese" {
            return Err(format!("Tab did not close search without committing: {tab}").into());
        }
        driver.select(
            "#language",
            original["language"]
                .as_str()
                .ok_or("Missing original language")?,
        )?;
        driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(2)")?;
        driver.click("[data-testid=queue-refresh]")?;
        driver.wait("return document.querySelector('[data-testid=queue-refresh]')?.disabled === false && document.querySelectorAll('.queue-row').length === 0;", 5)?;
        driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(1)")?;
        driver.wait("return document.querySelector('.source-frame')?.naturalWidth > 0 && document.querySelector('#extract-start')?.disabled === false;", 10)?;
        Ok(json!({
            "query": query_state, "no_match": no_match, "cleared": cleared,
            "listbox_scroll": scrolled, "scroll_bounds": after_scroll,
            "composition_event": composition, "navigation": moved, "cancelled": cancelled,
            "committed": committed, "pointer_choice": pointer_choice, "tab": tab,
            "native_queue_receipts": receipts,
            "physical_ime_and_native_tab_traversal_verified": false
        }))
    })();
    let receipt = result?;
    std::fs::write(
        output.join("language-selection.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}

fn query(driver: &Driver, value: &str) -> Result<()> {
    driver.script(&format!("const input=document.querySelector('#language-search');input.focus();input.value={};input.dispatchEvent(new Event('input',{{bubbles:true}}));return true;", serde_json::to_string(value)?))?;
    driver.wait(&format!("return document.querySelector('#language-search')?.value === {} && document.querySelector('#language-search')?.getAttribute('aria-expanded') === 'true';", serde_json::to_string(value)?), 5)
}

fn key(driver: &Driver, value: &str, composing: bool) -> Result<()> {
    driver.script(&format!("document.querySelector('#language-search').dispatchEvent(new KeyboardEvent('keydown',{{key:{},isComposing:{composing},bubbles:true,cancelable:true}}));return true;", serde_json::to_string(value)?))?;
    Ok(())
}

fn state(driver: &Driver) -> Result<Value> {
    driver.script(
        r#"
        const input = document.querySelector('#language-search'), select = document.querySelector('#language');
        const active = input.getAttribute('aria-activedescendant');
        const controlled = document.getElementById(input.getAttribute('aria-controls'));
        return {
            query: input.value, selected: select.value, options: select.options.length,
            role: input.getAttribute('role'), autocomplete: input.getAttribute('aria-autocomplete'),
            expanded: input.getAttribute('aria-expanded'), active,
            controlledRole: controlled?.getAttribute('role') ?? null,
            activeSelected: active ? document.getElementById(active)?.getAttribute('aria-selected') : null,
            focused: document.activeElement === input,
            matches: Array.from(document.querySelectorAll('#language-results [role=option]')).map(e => ({id:e.id, selected:e.getAttribute('aria-selected'), tabIndex:e.tabIndex})),
            emptyStatus: Array.from(document.querySelectorAll('.language-picker [role=status]')).some(e => e.textContent.includes('No matching language'))
        };
        "#,
    )
}

fn assert_selection(state: &Value, expected: &str, original: &Value) -> Result<()> {
    if state["selected"] != expected || state["options"] != original["options"] {
        return Err(
            format!("Committed language or full catalogue changed unexpectedly: {state}").into(),
        );
    }
    Ok(())
}

fn checkpoint(driver: &Driver, phase: &str, expected: &str) -> Result<Value> {
    if queue_entries(driver)?.as_array().map(Vec::len) != Some(0) {
        return Err("Language witness requires an empty isolated fixture queue".into());
    }
    driver.click("#extract-queue")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let entries = loop {
        let entries = queue_entries(driver)?;
        if entries
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
        {
            break entries;
        }
        let error =
            driver.script("return document.querySelector('[role=alert]')?.textContent || '';")?;
        if error.as_str().is_some_and(|error| !error.is_empty()) {
            return Err(format!("Language queue admission failed: {error}").into());
        }
        if Instant::now() >= deadline {
            return Err(
                format!("Native queue did not receive language selection at {phase}").into(),
            );
        }
        std::thread::park_timeout(Duration::from_millis(50));
    };
    driver.wait(
        "return document.querySelector('#extract-queue')?.disabled === false;",
        5,
    )?;
    if entries.as_array().map(Vec::len) != Some(1) || entries[0]["language"] != expected {
        return Err(format!(
            "Actual native queue received the wrong language at {phase}: {entries}"
        )
        .into());
    }
    let removed = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":"const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('queue_remove',{id:arguments[0]}).then(entries=>done({entries}),error=>done({error:String(error)}));","args":[entries[0]["id"]]}),
    )?;
    if removed["entries"].as_array().map(Vec::len) != Some(0) {
        return Err(format!("Language witness queue cleanup failed: {removed}").into());
    }
    Ok(json!({"phase":phase,"persisted_native_entry":entries[0]}))
}

fn queue_entries(driver: &Driver) -> Result<Value> {
    let result = driver.request(
        reqwest::Method::POST,
        "/execute/async",
        json!({"script":"const done=arguments[arguments.length-1];window.__TAURI__.core.invoke('queue_list').then(entries=>done({entries}),error=>done({error:String(error)}));","args":[]}),
    )?;
    if !result["entries"].is_array() {
        return Err(format!("Could not inspect native language configuration: {result}").into());
    }
    Ok(result["entries"].clone())
}
