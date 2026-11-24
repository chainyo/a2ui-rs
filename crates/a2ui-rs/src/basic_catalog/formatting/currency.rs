use std::fmt;

use icu_decimal::parts;
use icu_decimal::provider::{DecimalDigitsV1, DecimalSymbolsV1};
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::currency::formatter::CurrencyFormatter;
use icu_experimental::dimension::currency::options::CurrencyFormatterOptions;
use icu_experimental::dimension::provider::currency::{
    essentials::CurrencyEssentialsV1,
    fractions::{CurrencyFractions, CurrencyFractionsV1, FractionInfo},
    no_currency::CurrencyPatternsNoCurrencyV1,
    symbols::{CurrencyDecimalSymbolsV1, CurrencySymbolsV1},
};
use icu_locale_core::Locale;
use icu_provider::prelude::*;
use writeable::{Part, PartsWrite, Writeable};

use super::{JsonObject, decimals, number, use_grouping};

pub(super) fn format(locale: &Locale, args: &JsonObject) -> Result<String, String> {
    let value = number(&args["value"])?;
    let currency = args["currency"]
        .as_str()
        .ok_or("Expected a currency code")?;
    let currency = CurrencyType::try_from_str(&currency.to_ascii_lowercase())
        .map_err(|_| "Expected a three-letter currency code")?;
    let places = u8::try_from(decimals(args)?.unwrap_or(2)).map_err(|error| error.to_string())?;
    let formatter = CurrencyFormatter::try_new_symbol_unstable(
        &CurrencyData { places },
        locale.into(),
        currency,
        CurrencyFormatterOptions::default(),
    )
    .map_err(|error| error.to_string())?;
    let amount = formatter.format_fixed_decimal(&value);
    if use_grouping(args) {
        return Ok(amount.write_to_string().into_owned());
    }
    let mut output = UngroupedCurrency(String::new());
    amount
        .write_to_parts(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output.0)
}

struct CurrencyData {
    places: u8,
}

macro_rules! use_compiled_currency_data {
    ($provider:path; $($marker:ty),+ $(,)?) => {$(
        impl DataProvider<$marker> for CurrencyData {
            fn load(&self, request: DataRequest) -> Result<DataResponse<$marker>, DataError> {
                <$provider as DataProvider<$marker>>::load(&$provider, request)
            }
        }
    )+};
}

use_compiled_currency_data!(
    icu_experimental::provider::Baked;
    CurrencyEssentialsV1, CurrencyPatternsNoCurrencyV1, CurrencySymbolsV1, CurrencyDecimalSymbolsV1,
);
use_compiled_currency_data!(icu_decimal::provider::Baked; DecimalSymbolsV1, DecimalDigitsV1);

impl DataProvider<CurrencyFractionsV1> for CurrencyData {
    fn load(&self, _: DataRequest) -> Result<DataResponse<CurrencyFractionsV1>, DataError> {
        // Use the catalog precision for all currency codes.
        #[expect(
            clippy::default_trait_access,
            reason = "The map type is part of the ICU data API"
        )]
        let fractions = CurrencyFractions {
            fractions: Default::default(),
            default: FractionInfo {
                digits: self.places,
                ..FractionInfo::default()
            },
        };
        Ok(DataResponse {
            metadata: DataResponseMetadata::default(),
            payload: DataPayload::from_owned(fractions),
        })
    }
}

struct UngroupedCurrency(String);

impl fmt::Write for UngroupedCurrency {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.push_str(text);
        Ok(())
    }
}

impl PartsWrite for UngroupedCurrency {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        if part == parts::GROUP {
            return Ok(());
        }
        write(self)
    }
}
