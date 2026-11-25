use std::borrow::Cow;

use fixed_decimal::{Decimal, FloatPrecision, SignedRoundingMode, UnsignedRoundingMode};
use icu_calendar::Gregorian;
use icu_datetime::input::{Date, DateTime, Time};
use icu_datetime::pattern::{
    DateTimePattern, DateTimePatternFormatter, FixedCalendarDateTimeNames, PatternLoadError,
};
use icu_datetime::provider::pattern::{PatternItem, runtime::Pattern};
use icu_decimal::DecimalFormatter;
use icu_decimal::options::GroupingStrategy;
use icu_locale_core::Locale;
use icu_plurals::{PluralCategory, PluralRules};
use jiff::civil::Time as CivilTime;
use jiff::fmt::temporal::DateTimeParser;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, Zoned};
use serde_json::Value;
use thiserror::Error;
use writeable::TryWriteable;

use a2ui_rs::protocol::JsonObject;
use a2ui_rs::resolution::{LocalFunctions, to_boolean};

mod currency;

/// The locale and time zone for Basic Catalog formatting.
#[derive(Clone, Debug)]
pub struct FormattingOptions {
    locale: Locale,
    time_zone: TimeZone,
}

/// An invalid Basic Catalog formatting option.
#[derive(Debug, Error)]
pub enum FormattingOptionsError {
    /// The locale is not a BCP 47 language tag.
    #[error("Invalid formatting locale: {0}")]
    Locale(String),
    /// The time zone is not in the IANA time zone database.
    #[error("Unknown formatting time zone: {0}")]
    TimeZone(String),
}

impl FormattingOptions {
    /// Sets a BCP 47 locale and an IANA time zone.
    ///
    /// # Errors
    ///
    /// Returns an error if the locale or time zone is invalid.
    pub fn new(locale: &str, time_zone: &str) -> Result<Self, FormattingOptionsError> {
        Ok(Self {
            locale: locale
                .parse()
                .map_err(|_| FormattingOptionsError::Locale(locale.into()))?,
            time_zone: TimeZone::get(time_zone)
                .map_err(|_| FormattingOptionsError::TimeZone(time_zone.into()))?,
        })
    }
}

/// Registers the Basic Catalog formatting functions for a loaded v1.0 catalog.
///
/// The time zone applies to epoch values and date strings without an offset.
/// A date string with an offset uses that offset. The `ISO` format always uses UTC.
pub fn register_formatting_functions(
    functions: &mut LocalFunctions<'_>,
    catalog_id: &str,
    options: FormattingOptions,
) {
    let locale = options.locale.clone();
    functions.insert(catalog_id, "formatNumber", move |args| {
        format_number(&locale, args).map(Value::String)
    });
    let locale = options.locale.clone();
    functions.insert(catalog_id, "formatCurrency", move |args| {
        currency::format(&locale, args).map(Value::String)
    });
    let locale = options.locale.clone();
    functions.insert(catalog_id, "pluralize", move |args| {
        pluralize(&locale, args).map(Value::String)
    });
    functions.insert(catalog_id, "formatDate", move |args| {
        format_date(&options, args).map(Value::String)
    });
}

fn number(value: &Value) -> Result<Decimal, String> {
    if let Some(value) = value.as_i64() {
        return Ok(Decimal::from(value));
    }
    if let Some(value) = value.as_u64() {
        return Ok(Decimal::from(value));
    }
    let value = value.as_f64().ok_or("Expected a number")?;
    Decimal::try_from_f64(value, FloatPrecision::RoundTrip).map_err(|error| error.to_string())
}

fn decimals(args: &JsonObject) -> Result<Option<i16>, String> {
    let Some(value) = args.get("decimals") else {
        return Ok(None);
    };
    let value = value
        .as_f64()
        .ok_or("Expected a decimal place count")?
        .trunc();
    if !(0.0..=100.0).contains(&value) {
        return Err("Decimal place count must be between 0 and 100".into());
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "The count is an integer in range"
    )]
    let value = value as i16;
    Ok(Some(value))
}

fn use_grouping(args: &JsonObject) -> bool {
    args.get("grouping").is_none_or(to_boolean)
}

fn decimal_formatter(locale: &Locale, grouping: bool) -> Result<DecimalFormatter, String> {
    let options = if grouping {
        GroupingStrategy::Always
    } else {
        GroupingStrategy::Never
    };
    DecimalFormatter::try_new(locale.into(), options.into()).map_err(|error| error.to_string())
}

fn format_number(locale: &Locale, args: &JsonObject) -> Result<String, String> {
    let value = number(&args["value"])?;
    let places = decimals(args)?;
    let mut value = value.rounded_with_mode(
        -places.unwrap_or(3),
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    );
    if let Some(places) = places {
        value.absolute.pad_end(-places);
    } else {
        value.absolute.trim_end();
    }
    Ok(decimal_formatter(locale, use_grouping(args))?
        .format(&value)
        .to_string())
}

fn pluralize(locale: &Locale, args: &JsonObject) -> Result<String, String> {
    let value = number(&args["value"])?;
    let rules = PluralRules::try_new_cardinal(locale.into()).map_err(|error| error.to_string())?;
    let exact = match args["value"].as_f64() {
        Some(0.0) => args.get("zero"),
        Some(1.0) => args.get("one"),
        Some(2.0) => args.get("two"),
        _ => None,
    };
    let category = match rules.category_for(&value) {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        PluralCategory::Other => "other",
    };
    exact
        .or_else(|| args.get(category))
        .or_else(|| args.get("other"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Expected a plural form".into())
}

fn format_date(options: &FormattingOptions, args: &JsonObject) -> Result<String, String> {
    let pattern = args["format"].as_str().ok_or("Expected a date pattern")?;
    let pattern = if pattern.is_empty() {
        "yyyy-MM-dd"
    } else {
        pattern
    };
    let Some(zoned) = parse_date(&args["value"], &options.time_zone) else {
        return Ok(String::new());
    };
    if pattern == "ISO" {
        let timestamp = zoned.timestamp();
        return Ok(format!("{timestamp:.3}"));
    }
    let pattern: DateTimePattern = pattern
        .parse()
        .map_err(|_| "Expected a Unicode TR35 date pattern")?;
    let civil = zoned.datetime();
    let datetime = DateTime {
        date: Date::try_new_gregorian(
            civil.year().into(),
            civil.month().cast_unsigned(),
            civil.day().cast_unsigned(),
        )
        .map_err(|error| error.to_string())?,
        time: Time::try_new(
            civil.hour().cast_unsigned(),
            civil.minute().cast_unsigned(),
            civil.second().cast_unsigned(),
            civil.subsec_nanosecond().cast_unsigned(),
        )
        .map_err(|error| error.to_string())?,
    };
    let new_names = || {
        FixedCalendarDateTimeNames::<Gregorian>::try_new((&options.locale).into())
            .map_err(|error| error.to_string())
    };
    let format = |formatter: DateTimePatternFormatter<'_, Gregorian, _>| {
        formatter
            .format(&datetime)
            .try_write_to_string()
            .map(Cow::into_owned)
            .map_err(|(error, _)| error.to_string())
    };
    let mut names = new_names()?;
    match names.include_for_pattern(&pattern) {
        Ok(formatter) => return format(formatter),
        Err(PatternLoadError::ConflictingField { .. }) => {}
        Err(error) => return Err(error.to_string()),
    }

    // ICU cannot load two widths of the same name in one formatter.
    let pattern = Pattern::from(pattern);
    let mut output = String::new();
    for item in pattern.items.iter() {
        if let PatternItem::Literal(character) = item {
            output.push(character);
        } else {
            let pattern = DateTimePattern::from(Pattern::from_iter([item]));
            let mut names = new_names()?;
            let formatter = names
                .include_for_pattern(&pattern)
                .map_err(|error| error.to_string())?;
            output.push_str(&format(formatter)?);
        }
    }
    Ok(output)
}

fn parse_date(value: &Value, time_zone: &TimeZone) -> Option<Zoned> {
    if let Some(text) = value.as_str() {
        let pieces = DateTimeParser::new().parse_pieces(text).ok()?;
        let zone = pieces
            .to_numeric_offset()
            .map_or_else(|| time_zone.clone(), TimeZone::fixed);
        let time = pieces.time().unwrap_or_else(CivilTime::midnight);
        return pieces.date().to_datetime(time).to_zoned(zone).ok();
    }
    let epoch = value.as_f64()?;
    let seconds = if epoch.abs() > 10_000_000_000.0 {
        epoch / 1000.0
    } else {
        epoch
    };
    let duration = SignedDuration::try_from_secs_f64(seconds).ok()?;
    let timestamp = Timestamp::UNIX_EPOCH.checked_add(duration).ok()?;
    Some(timestamp.to_zoned(time_zone.clone()))
}
