//! Offline place names for the Photo World Map: about eight hundred of the world's cities and the countries
//! they are in (a 24 KB table in the program), with a nearest-place lookup that never scans them all.
//!
//! Coordinates are rounded to 0.01 degrees. A city's rank says how early the map labels it: 1 is a world
//! city (labelled from the first zoom steps), 2 a large one, 3 a smaller one (labelled once zoomed in).
//! The lookup keeps the cities in a grid of 5 degree cells and searches outward from the cell asked about,
//! so a cluster header ("42 photos near Tokyo") costs a handful of cells, not the whole table, and the map
//! asks once per cluster per camera change, never per frame.

use crate::world_map::{project, MapCamera};
use std::sync::LazyLock;

/// One city.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct City {
    pub name: &'static str,
    /// Index into [`Gazetteer::countries`].
    pub country: u16,
    pub lat: f32,
    pub lon: f32,
    /// Where it is on the world map (`world_map::project`), worked out once.
    pub wx: f64,
    pub wy: f64,
    /// 1 (world city), 2 (large) or 3 (smaller).
    pub rank: u8,
}

/// One country, where its name is drawn on the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Country {
    pub code: &'static str,
    pub name: &'static str,
    pub lat: f32,
    pub lon: f32,
}

/// The nearest city to a point, and how far it is.
#[derive(Clone, Copy, Debug)]
pub struct Nearest<'a> {
    pub city: &'a City,
    pub km: f32,
}

/// The table and its grid.
pub struct Gazetteer {
    countries: Vec<Country>,
    cities: Vec<City>,
    /// `cells[cell]` is the range of `order` holding that cell's cities.
    cells: Vec<(u32, u32)>,
    order: Vec<u16>,
}

const CELL: f32 = 5.0;
const COLS: i32 = 72;
const ROWS: i32 = 36;

/// Countries and their cities: `CC Country lat lon | City lat lon rank, City lat lon rank, ...`.
const DATA: &str = "\
JP Japan 36.20 138.25 | Tokyo 35.69 139.69 1, Osaka 34.69 135.50 1, Kyoto 35.01 135.77 2, Yokohama 35.44 139.64 2, Nagoya 35.18 136.91 2, Sapporo 43.06 141.35 2, Fukuoka 33.59 130.40 2, Hiroshima 34.39 132.46 3, Sendai 38.27 140.87 3, Naha 26.21 127.68 3, Kobe 34.69 135.20 3, Nara 34.69 135.80 3, Kanazawa 36.56 136.66 3, Nagasaki 32.75 129.88 3, Hakodate 41.77 140.73 3, Kagoshima 31.60 130.56 3
CN China 35.00 103.00 | Beijing 39.90 116.41 1, Shanghai 31.23 121.47 1, Guangzhou 23.13 113.26 1, Shenzhen 22.54 114.06 1, Chengdu 30.57 104.07 2, Chongqing 29.56 106.55 2, Wuhan 30.59 114.31 2, Xi'an 34.34 108.94 2, Hangzhou 30.27 120.15 2, Nanjing 32.06 118.80 2, Tianjin 39.34 117.36 2, Harbin 45.80 126.53 3, Kunming 25.04 102.71 3, Qingdao 36.07 120.38 3, Lhasa 29.65 91.17 3, Urumqi 43.83 87.62 3, Guilin 25.27 110.29 3, Xiamen 24.48 118.09 3, Lanzhou 36.06 103.83 3, Shenyang 41.80 123.43 3
HK Hong Kong 22.32 114.17 | Hong Kong 22.32 114.17 1
MO Macau 22.20 113.54 | Macau 22.20 113.54 3
TW Taiwan 23.70 121.00 | Taipei 25.03 121.57 1, Kaohsiung 22.63 120.30 3, Taichung 24.15 120.67 3
KR South Korea 36.50 127.90 | Seoul 37.57 126.98 1, Busan 35.18 129.08 2, Incheon 37.46 126.71 3, Jeju 33.50 126.53 3, Daegu 35.87 128.60 3
KP North Korea 40.30 127.20 | Pyongyang 39.04 125.76 3
MN Mongolia 46.90 103.80 | Ulaanbaatar 47.89 106.91 3
IN India 22.50 79.00 | Delhi 28.61 77.21 1, Mumbai 19.08 72.88 1, Bengaluru 12.97 77.59 1, Kolkata 22.57 88.36 1, Chennai 13.08 80.27 2, Hyderabad 17.39 78.49 2, Ahmedabad 23.02 72.57 2, Pune 18.52 73.86 2, Jaipur 26.91 75.79 2, Lucknow 26.85 80.95 3, Varanasi 25.32 83.01 3, Agra 27.18 78.01 3, Goa 15.50 73.83 3, Kochi 9.93 76.27 3, Amritsar 31.63 74.87 3, Udaipur 24.59 73.71 3, Leh 34.16 77.58 3, Srinagar 34.08 74.80 3, Thiruvananthapuram 8.52 76.94 3, Bhopal 23.26 77.41 3
PK Pakistan 30.00 70.00 | Karachi 24.86 67.01 1, Lahore 31.55 74.34 2, Islamabad 33.69 73.06 2, Peshawar 34.01 71.58 3, Quetta 30.18 66.99 3
BD Bangladesh 23.80 90.30 | Dhaka 23.81 90.41 1, Chittagong 22.34 91.83 3
LK Sri Lanka 7.60 80.70 | Colombo 6.93 79.86 2, Kandy 7.29 80.63 3
NP Nepal 28.30 84.10 | Kathmandu 27.72 85.32 2, Pokhara 28.21 83.99 3
BT Bhutan 27.50 90.40 | Thimphu 27.47 89.64 3
MV Maldives 3.20 73.20 | Male 4.18 73.51 3
AF Afghanistan 33.90 67.70 | Kabul 34.53 69.17 2, Kandahar 31.63 65.74 3
TH Thailand 15.00 101.00 | Bangkok 13.76 100.50 1, Chiang Mai 18.79 98.98 2, Phuket 7.88 98.39 3, Pattaya 12.93 100.88 3, Krabi 8.09 98.91 3, Hat Yai 7.01 100.47 3, Koh Samui 9.51 100.01 3
VN Vietnam 16.00 107.80 | Ho Chi Minh City 10.82 106.63 1, Hanoi 21.03 105.85 1, Da Nang 16.05 108.21 2, Hue 16.46 107.59 3, Hoi An 15.88 108.34 3, Nha Trang 12.24 109.19 3, Da Lat 11.94 108.44 3, Sapa 22.34 103.84 3
KH Cambodia 12.60 104.90 | Phnom Penh 11.56 104.93 2, Siem Reap 13.36 103.86 3
LA Laos 18.20 103.90 | Vientiane 17.97 102.63 3, Luang Prabang 19.89 102.13 3
MM Myanmar 21.00 96.00 | Yangon 16.84 96.17 2, Mandalay 21.98 96.08 3, Naypyidaw 19.76 96.08 3
MY Malaysia 4.20 102.00 | Kuala Lumpur 3.14 101.69 1, George Town 5.41 100.33 3, Kota Kinabalu 5.98 116.07 3, Kuching 1.55 110.35 3, Johor Bahru 1.49 103.74 3, Malacca 2.19 102.25 3
SG Singapore 1.35 103.82 | Singapore 1.35 103.82 1
ID Indonesia -2.50 118.00 | Jakarta -6.21 106.85 1, Surabaya -7.25 112.75 2, Bandung -6.92 107.61 2, Medan 3.59 98.67 2, Denpasar -8.65 115.22 2, Yogyakarta -7.80 110.36 3, Makassar -5.15 119.43 3, Manado 1.47 124.84 3, Balikpapan -1.24 116.83 3, Mataram -8.58 116.12 3, Padang -0.95 100.35 3, Jayapura -2.53 140.72 3
PH Philippines 12.90 122.90 | Manila 14.60 120.98 1, Cebu 10.32 123.89 2, Davao 7.07 125.61 2, Baguio 16.40 120.59 3, Puerto Princesa 9.74 118.74 3, Iloilo 10.72 122.56 3
BN Brunei 4.50 114.70 | Bandar Seri Begawan 4.94 114.95 3
TL Timor-Leste -8.80 125.70 | Dili -8.56 125.57 3
AU Australia -25.30 133.80 | Sydney -33.87 151.21 1, Melbourne -37.81 144.96 1, Brisbane -27.47 153.03 2, Perth -31.95 115.86 2, Adelaide -34.93 138.60 2, Canberra -35.28 149.13 2, Gold Coast -28.02 153.40 3, Hobart -42.88 147.33 3, Darwin -12.46 130.84 3, Cairns -16.92 145.77 3, Alice Springs -23.70 133.88 3, Newcastle -32.93 151.78 3, Broome -17.96 122.24 3, Townsville -19.26 146.82 3
NZ New Zealand -41.00 174.00 | Auckland -36.85 174.76 1, Wellington -41.29 174.78 2, Christchurch -43.53 172.64 2, Queenstown -45.03 168.66 3, Dunedin -45.87 170.50 3, Rotorua -38.14 176.25 3, Napier -39.49 176.91 3, Nelson -41.27 173.28 3
PG Papua New Guinea -6.30 143.90 | Port Moresby -9.44 147.18 3
FJ Fiji -17.70 178.00 | Suva -18.14 178.44 3, Nadi -17.80 177.42 3
PF French Polynesia -17.70 -149.40 | Papeete -17.54 -149.57 3
NC New Caledonia -21.30 165.50 | Noumea -22.28 166.46 3
WS Samoa -13.80 -172.10 | Apia -13.83 -171.76 3
TO Tonga -21.20 -175.20 | Nuku'alofa -21.14 -175.20 3
VU Vanuatu -15.40 166.90 | Port Vila -17.73 168.32 3
GU Guam 13.44 144.79 | Hagatna 13.48 144.75 3
US United States 39.80 -98.60 | New York 40.71 -74.01 1, Los Angeles 34.05 -118.24 1, Chicago 41.88 -87.63 1, San Francisco 37.77 -122.42 1, Washington 38.91 -77.04 1, Houston 29.76 -95.37 2, Miami 25.76 -80.19 2, Seattle 47.61 -122.33 2, Boston 42.36 -71.06 2, Dallas 32.78 -96.80 2, Atlanta 33.75 -84.39 2, Las Vegas 36.17 -115.14 2, San Diego 32.72 -117.16 2, Denver 39.74 -104.99 2, Phoenix 33.45 -112.07 2, Philadelphia 39.95 -75.17 2, Honolulu 21.31 -157.86 2, Anchorage 61.22 -149.90 3, Orlando 28.54 -81.38 3, New Orleans 29.95 -90.07 3, Nashville 36.16 -86.78 3, Austin 30.27 -97.74 3, Portland 45.52 -122.68 3, Minneapolis 44.98 -93.27 3, Detroit 42.33 -83.05 3, Salt Lake City 40.76 -111.89 3, Kansas City 39.10 -94.58 3, St. Louis 38.63 -90.20 3, Pittsburgh 40.44 -80.00 3, Charlotte 35.23 -80.84 3, San Antonio 29.42 -98.49 3, Santa Fe 35.69 -105.94 3, Albuquerque 35.08 -106.65 3, Sacramento 38.58 -121.49 3, Cleveland 41.50 -81.69 3, Indianapolis 39.77 -86.16 3, Savannah 32.08 -81.09 3, Charleston 32.78 -79.93 3, Tampa 27.95 -82.46 3, Buffalo 42.89 -78.88 3, Jackson 43.48 -110.76 3, Fairbanks 64.84 -147.72 3, Juneau 58.30 -134.42 3, Boise 43.62 -116.20 3, Omaha 41.26 -95.93 3, Memphis 35.15 -90.05 3, Louisville 38.25 -85.76 3
CA Canada 56.10 -106.30 | Toronto 43.65 -79.38 1, Vancouver 49.28 -123.12 1, Montreal 45.50 -73.57 1, Calgary 51.05 -114.07 2, Ottawa 45.42 -75.70 2, Edmonton 53.55 -113.49 2, Winnipeg 49.90 -97.14 3, Quebec City 46.81 -71.21 3, Halifax 44.65 -63.57 3, Victoria 48.43 -123.37 3, Saskatoon 52.13 -106.67 3, St. John's 47.56 -52.71 3, Whitehorse 60.72 -135.06 3, Yellowknife 62.45 -114.37 3, Banff 51.18 -115.57 3, Regina 50.45 -104.62 3, Iqaluit 63.75 -68.52 3
MX Mexico 23.60 -102.50 | Mexico City 19.43 -99.13 1, Guadalajara 20.67 -103.35 2, Monterrey 25.69 -100.32 2, Cancun 21.16 -86.85 2, Tijuana 32.51 -117.04 3, Puebla 19.04 -98.21 3, Mérida 20.97 -89.62 3, Oaxaca 17.07 -96.73 3, León 21.12 -101.68 3, Acapulco 16.85 -99.82 3, La Paz 24.14 -110.31 3, Veracruz 19.17 -96.13 3, Playa del Carmen 20.63 -87.07 3, Puerto Vallarta 20.65 -105.23 3, San Cristobal de las Casas 16.74 -92.64 3, Cabo San Lucas 22.89 -109.91 3
GT Guatemala 15.70 -90.20 | Guatemala City 14.63 -90.51 3, Antigua Guatemala 14.56 -90.73 3
BZ Belize 17.20 -88.70 | Belize City 17.50 -88.20 3
SV El Salvador 13.70 -88.90 | San Salvador 13.69 -89.22 3
HN Honduras 14.80 -86.60 | Tegucigalpa 14.07 -87.19 3
NI Nicaragua 12.90 -85.00 | Managua 12.11 -86.24 3
CR Costa Rica 9.90 -84.20 | San Jose 9.93 -84.08 3, Liberia 10.63 -85.44 3
PA Panama 8.50 -80.10 | Panama City 8.98 -79.52 2
CU Cuba 21.60 -79.00 | Havana 23.11 -82.37 2, Santiago de Cuba 20.02 -75.83 3, Varadero 23.15 -81.25 3
JM Jamaica 18.10 -77.30 | Kingston 17.97 -76.79 3, Montego Bay 18.48 -77.92 3
HT Haiti 19.00 -72.40 | Port-au-Prince 18.54 -72.34 3
DO Dominican Republic 18.90 -70.50 | Santo Domingo 18.49 -69.93 3, Punta Cana 18.58 -68.40 3
PR Puerto Rico 18.20 -66.50 | San Juan 18.47 -66.11 3
BS Bahamas 24.70 -77.80 | Nassau 25.05 -77.35 3
BB Barbados 13.19 -59.54 | Bridgetown 13.10 -59.61 3
TT Trinidad and Tobago 10.60 -61.30 | Port of Spain 10.65 -61.52 3
AW Aruba 12.52 -69.97 | Oranjestad 12.52 -70.03 3
CO Colombia 4.60 -74.30 | Bogotá 4.71 -74.07 1, Medellín 6.24 -75.58 2, Cartagena 10.39 -75.48 2, Cali 3.45 -76.53 3, Barranquilla 10.96 -74.80 3
VE Venezuela 7.00 -66.00 | Caracas 10.48 -66.90 2, Maracaibo 10.63 -71.64 3
EC Ecuador -1.80 -78.20 | Quito -0.18 -78.47 2, Guayaquil -2.17 -79.92 3, Cuenca -2.90 -79.00 3
PE Peru -9.20 -75.00 | Lima -12.05 -77.04 1, Cusco -13.53 -71.97 2, Arequipa -16.41 -71.54 3, Iquitos -3.75 -73.25 3, Trujillo -8.11 -79.03 3
BO Bolivia -16.30 -63.60 | La Paz -16.50 -68.15 2, Santa Cruz -17.78 -63.18 3, Sucre -19.03 -65.26 3, Uyuni -20.46 -66.82 3
BR Brazil -10.00 -53.00 | São Paulo -23.55 -46.63 1, Rio de Janeiro -22.91 -43.17 1, Brasília -15.79 -47.88 2, Salvador -12.97 -38.50 2, Fortaleza -3.73 -38.52 2, Belo Horizonte -19.92 -43.94 2, Manaus -3.12 -60.02 2, Curitiba -25.43 -49.27 2, Recife -8.05 -34.88 2, Porto Alegre -30.03 -51.23 2, Belém -1.46 -48.50 3, Florianopolis -27.60 -48.55 3, Natal -5.79 -35.21 3, Foz do Iguacu -25.55 -54.59 3, Goiania -16.69 -49.26 3, Campo Grande -20.47 -54.62 3, Cuiaba -15.60 -56.10 3, Porto Velho -8.76 -63.90 3
AR Argentina -34.00 -64.00 | Buenos Aires -34.60 -58.38 1, Córdoba -31.42 -64.18 2, Rosario -32.95 -60.64 3, Mendoza -32.89 -68.83 3, Bariloche -41.13 -71.31 3, Ushuaia -54.80 -68.30 3, Salta -24.78 -65.41 3, El Calafate -50.34 -72.28 3, Mar del Plata -38.00 -57.56 3
CL Chile -35.70 -71.50 | Santiago -33.45 -70.67 1, Valparaiso -33.05 -71.62 3, Punta Arenas -53.16 -70.91 3, San Pedro de Atacama -22.91 -68.20 3, Puerto Montt -41.47 -72.94 3, Antofagasta -23.65 -70.40 3, Easter Island -27.11 -109.35 3
UY Uruguay -32.50 -55.80 | Montevideo -34.90 -56.16 2, Punta del Este -34.96 -54.95 3
PY Paraguay -23.40 -58.40 | Asuncion -25.26 -57.58 3
GF French Guiana 4.00 -53.00 | Cayenne 4.92 -52.33 3
SR Suriname 4.00 -56.00 | Paramaribo 5.85 -55.20 3
GY Guyana 5.00 -59.00 | Georgetown 6.80 -58.16 3
GB United Kingdom 54.00 -2.50 | London 51.51 -0.13 1, Manchester 53.48 -2.24 2, Birmingham 52.49 -1.89 2, Edinburgh 55.95 -3.19 2, Glasgow 55.86 -4.25 2, Liverpool 53.41 -2.98 3, Bristol 51.45 -2.59 3, Cardiff 51.48 -3.18 3, Belfast 54.60 -5.93 3, Leeds 53.80 -1.55 3, Newcastle 54.98 -1.62 3, Oxford 51.75 -1.26 3, Cambridge 52.21 0.12 3, Aberdeen 57.15 -2.09 3, Inverness 57.48 -4.22 3, Brighton 50.82 -0.14 3, Bath 51.38 -2.36 3, York 53.96 -1.08 3, Plymouth 50.38 -4.14 3, Southampton 50.90 -1.40 3, Nottingham 52.95 -1.15 3, Sheffield 53.38 -1.47 3, Lerwick 60.15 -1.15 3, Gibraltar 36.14 -5.35 3
IE Ireland 53.20 -8.00 | Dublin 53.35 -6.26 2, Cork 51.90 -8.47 3, Galway 53.27 -9.05 3, Limerick 52.67 -8.63 3
FR France 46.60 2.40 | Paris 48.86 2.35 1, Marseille 43.30 5.37 2, Lyon 45.76 4.84 2, Toulouse 43.60 1.44 2, Nice 43.70 7.27 2, Bordeaux 44.84 -0.58 3, Strasbourg 48.57 7.75 3, Nantes 47.22 -1.55 3, Lille 50.63 3.06 3, Montpellier 43.61 3.88 3, Rennes 48.11 -1.68 3, Cannes 43.55 7.01 3, Avignon 43.95 4.81 3, Biarritz 43.48 -1.56 3, Chamonix 45.92 6.87 3, Ajaccio 41.92 8.74 3, Reims 49.26 4.03 3, Dijon 47.32 5.04 3, Grenoble 45.19 5.72 3, Caen 49.18 -0.37 3
MC Monaco 43.74 7.42 | Monaco 43.74 7.42 3
ES Spain 40.20 -3.70 | Madrid 40.42 -3.70 1, Barcelona 41.39 2.17 1, Valencia 39.47 -0.38 2, Seville 37.39 -5.98 2, Málaga 36.72 -4.42 3, Bilbao 43.26 -2.93 3, Granada 37.18 -3.60 3, Palma 39.57 2.65 3, Zaragoza 41.65 -0.88 3, San Sebastian 43.32 -1.98 3, Santiago de Compostela 42.88 -8.54 3, Las Palmas 28.12 -15.43 3, Santa Cruz de Tenerife 28.46 -16.25 3, Córdoba 37.89 -4.78 3, Alicante 38.35 -0.48 3, Ibiza 38.91 1.43 3, Toledo 39.86 -4.02 3, Salamanca 40.97 -5.66 3, Murcia 37.99 -1.13 3
PT Portugal 39.60 -8.00 | Lisbon 38.72 -9.14 2, Porto 41.15 -8.61 2, Faro 37.02 -7.93 3, Funchal 32.65 -16.91 3, Coimbra 40.21 -8.43 3, Ponta Delgada 37.74 -25.67 3, Sintra 38.80 -9.38 3
AD Andorra 42.50 1.52 | Andorra la Vella 42.51 1.52 3
IT Italy 42.80 12.50 | Rome 41.90 12.50 1, Milan 45.46 9.19 1, Naples 40.85 14.27 2, Venice 45.44 12.32 2, Florence 43.77 11.26 2, Turin 45.07 7.69 2, Bologna 44.49 11.34 3, Palermo 38.12 13.36 3, Genoa 44.41 8.93 3, Verona 45.44 10.99 3, Catania 37.50 15.09 3, Pisa 43.72 10.40 3, Bari 41.12 16.87 3, Cagliari 39.22 9.12 3, Siena 43.32 11.33 3, Bolzano 46.50 11.35 3, Trieste 45.65 13.78 3, Sorrento 40.63 14.38 3, Cortina d'Ampezzo 46.54 12.14 3, Lecce 40.35 18.17 3
VA Vatican City 41.90 12.45 | Vatican City 41.90 12.45 3
SM San Marino 43.94 12.46 | San Marino 43.94 12.46 3
MT Malta 35.90 14.40 | Valletta 35.90 14.51 3
DE Germany 51.20 10.40 | Berlin 52.52 13.40 1, Munich 48.14 11.58 2, Hamburg 53.55 9.99 2, Frankfurt 50.11 8.68 2, Cologne 50.94 6.96 2, Stuttgart 48.78 9.18 3, Düsseldorf 51.23 6.78 3, Dresden 51.05 13.74 3, Leipzig 51.34 12.37 3, Nuremberg 49.45 11.08 3, Hanover 52.37 9.73 3, Bremen 53.08 8.80 3, Heidelberg 49.40 8.69 3, Freiburg 47.99 7.85 3, Rostock 54.09 12.10 3, Garmisch-Partenkirchen 47.49 11.10 3, Trier 49.76 6.64 3, Dortmund 51.51 7.47 3
AT Austria 47.60 14.10 | Vienna 48.21 16.37 1, Salzburg 47.80 13.04 3, Innsbruck 47.27 11.39 3, Graz 47.07 15.44 3, Hallstatt 47.56 13.65 3, Linz 48.31 14.29 3
CH Switzerland 46.80 8.20 | Zürich 47.38 8.54 2, Geneva 46.20 6.14 2, Bern 46.95 7.45 3, Basel 47.56 7.59 3, Lucerne 47.05 8.31 3, Zermatt 46.02 7.75 3, Interlaken 46.69 7.86 3, Lausanne 46.52 6.63 3, Lugano 46.01 8.96 3, St. Moritz 46.49 9.84 3
LI Liechtenstein 47.17 9.55 | Vaduz 47.14 9.52 3
NL Netherlands 52.20 5.30 | Amsterdam 52.37 4.90 1, Rotterdam 51.92 4.48 2, The Hague 52.08 4.31 3, Utrecht 52.09 5.12 3, Eindhoven 51.44 5.48 3, Groningen 53.22 6.57 3, Maastricht 50.85 5.69 3
BE Belgium 50.60 4.60 | Brussels 50.85 4.35 2, Antwerp 51.22 4.40 3, Bruges 51.21 3.22 3, Ghent 51.05 3.72 3, Liege 50.63 5.57 3
LU Luxembourg 49.80 6.10 | Luxembourg 49.61 6.13 3
DK Denmark 56.00 10.00 | Copenhagen 55.68 12.57 2, Aarhus 56.16 10.20 3, Odense 55.40 10.39 3, Aalborg 57.05 9.92 3
NO Norway 62.00 10.00 | Oslo 59.91 10.75 2, Bergen 60.39 5.32 3, Tromso 69.65 18.96 3, Trondheim 63.43 10.40 3, Stavanger 58.97 5.73 3, Alesund 62.47 6.15 3, Bodo 67.28 14.40 3, Longyearbyen 78.22 15.65 3, Hammerfest 70.66 23.68 3, Kirkenes 69.73 30.05 3
SE Sweden 62.00 15.00 | Stockholm 59.33 18.07 2, Gothenburg 57.71 11.97 2, Malmo 55.60 13.00 3, Uppsala 59.86 17.64 3, Kiruna 67.86 20.23 3, Visby 57.64 18.29 3, Lund 55.70 13.19 3
FI Finland 64.00 26.00 | Helsinki 60.17 24.94 2, Tampere 61.50 23.76 3, Turku 60.45 22.27 3, Rovaniemi 66.50 25.73 3, Oulu 65.01 25.47 3, Ivalo 68.66 27.54 3
IS Iceland 64.90 -18.50 | Reykjavík 64.15 -21.94 2, Akureyri 65.69 -18.09 3, Vik 63.42 -19.01 3, Egilsstadir 65.27 -14.40 3, Hofn 64.25 -15.20 3, Isafjordur 66.07 -23.13 3
FO Faroe Islands 62.00 -6.80 | Torshavn 62.01 -6.77 3
GL Greenland 72.00 -40.00 | Nuuk 64.18 -51.72 3, Ilulissat 69.22 -51.10 3
PL Poland 52.00 19.40 | Warsaw 52.23 21.01 2, Krakow 50.06 19.94 2, Gdansk 54.35 18.65 3, Wroclaw 51.11 17.04 3, Poznan 52.41 16.93 3, Lodz 51.76 19.46 3, Zakopane 49.30 19.95 3, Lublin 51.25 22.57 3
CZ Czechia 49.80 15.50 | Prague 50.08 14.44 2, Brno 49.20 16.61 3, Cesky Krumlov 48.81 14.32 3, Ostrava 49.82 18.26 3
SK Slovakia 48.70 19.50 | Bratislava 48.15 17.11 3, Kosice 48.72 21.26 3
HU Hungary 47.20 19.50 | Budapest 47.50 19.04 2, Debrecen 47.53 21.63 3, Szeged 46.25 20.15 3
SI Slovenia 46.10 14.80 | Ljubljana 46.06 14.51 3, Bled 46.37 14.11 3
HR Croatia 45.10 15.20 | Zagreb 45.81 15.98 3, Split 43.51 16.44 3, Dubrovnik 42.65 18.09 3, Zadar 44.12 15.23 3, Pula 44.87 13.85 3
BA Bosnia and Herzegovina 44.00 17.80 | Sarajevo 43.86 18.41 3, Mostar 43.34 17.81 3
RS Serbia 44.00 21.00 | Belgrade 44.79 20.45 2, Novi Sad 45.26 19.83 3
ME Montenegro 42.70 19.30 | Podgorica 42.44 19.26 3, Kotor 42.42 18.77 3
MK North Macedonia 41.60 21.70 | Skopje 42.00 21.43 3, Ohrid 41.12 20.80 3
AL Albania 41.10 20.10 | Tirana 41.33 19.82 3, Sarande 39.87 20.01 3
XK Kosovo 42.60 20.90 | Pristina 42.66 21.17 3
GR Greece 39.00 22.00 | Athens 37.98 23.73 2, Thessaloniki 40.64 22.94 3, Heraklion 35.34 25.14 3, Santorini 36.39 25.46 3, Rhodes 36.43 28.22 3, Corfu 39.62 19.92 3, Mykonos 37.45 25.33 3, Chania 35.51 24.02 3, Patras 38.25 21.73 3, Kalamata 37.04 22.11 3, Meteora 39.72 21.63 3
BG Bulgaria 42.70 25.50 | Sofia 42.70 23.32 2, Plovdiv 42.14 24.75 3, Varna 43.21 27.91 3, Burgas 42.50 27.47 3
RO Romania 45.90 25.00 | Bucharest 44.43 26.10 2, Cluj-Napoca 46.77 23.59 3, Brasov 45.65 25.61 3, Timisoara 45.75 21.23 3, Constanta 44.18 28.65 3, Sibiu 45.80 24.15 3
MD Moldova 47.20 28.50 | Chisinau 47.01 28.86 3
UA Ukraine 49.00 31.40 | Kyiv 50.45 30.52 2, Kharkiv 49.99 36.23 3, Odesa 46.48 30.73 3, Lviv 49.84 24.03 3, Dnipro 48.46 35.05 3, Zaporizhzhia 47.84 35.14 3
BY Belarus 53.70 27.90 | Minsk 53.90 27.57 3
EE Estonia 58.70 25.50 | Tallinn 59.44 24.75 3, Tartu 58.38 26.72 3
LV Latvia 56.90 24.60 | Riga 56.95 24.11 3
LT Lithuania 55.30 23.90 | Vilnius 54.69 25.28 3, Kaunas 54.90 23.90 3
RU Russia 61.50 99.00 | Moscow 55.76 37.62 1, Saint Petersburg 59.93 30.34 2, Novosibirsk 55.03 82.92 3, Yekaterinburg 56.84 60.61 3, Kazan 55.80 49.11 3, Nizhny Novgorod 56.33 44.00 3, Sochi 43.60 39.73 3, Vladivostok 43.12 131.89 3, Irkutsk 52.29 104.30 3, Krasnoyarsk 56.01 92.85 3, Murmansk 68.97 33.08 3, Kaliningrad 54.71 20.51 3, Yakutsk 62.04 129.74 3, Omsk 54.99 73.37 3, Magadan 59.56 150.80 3, Petropavlovsk-Kamchatsky 53.04 158.65 3, Volgograd 48.71 44.51 3, Rostov-on-Don 47.24 39.71 3, Samara 53.20 50.15 3, Khabarovsk 48.48 135.08 3
TR Turkey 39.00 35.20 | Istanbul 41.01 28.98 1, Ankara 39.93 32.86 2, Izmir 38.42 27.14 2, Antalya 36.90 30.70 2, Bursa 40.19 29.06 3, Bodrum 37.04 27.43 3, Cappadocia 38.64 34.83 3, Trabzon 41.00 39.72 3, Gaziantep 37.07 37.38 3, Adana 37.00 35.32 3, Mardin 37.31 40.74 3, Konya 37.87 32.48 3, Fethiye 36.62 29.12 3, Van 38.49 43.38 3, Erzurum 39.90 41.27 3
CY Cyprus 35.00 33.40 | Nicosia 35.19 33.38 3, Limassol 34.68 33.04 3, Paphos 34.78 32.42 3
GE Georgia 42.30 43.40 | Tbilisi 41.72 44.79 3, Batumi 41.65 41.64 3, Kutaisi 42.27 42.70 3
AM Armenia 40.20 44.90 | Yerevan 40.18 44.51 3
AZ Azerbaijan 40.40 47.70 | Baku 40.41 49.87 2
KZ Kazakhstan 48.00 67.00 | Almaty 43.24 76.89 2, Astana 51.17 71.43 3, Shymkent 42.32 69.59 3
UZ Uzbekistan 41.40 64.60 | Tashkent 41.30 69.24 2, Samarkand 39.65 66.96 3, Bukhara 39.77 64.42 3, Khiva 41.38 60.36 3
KG Kyrgyzstan 41.50 74.60 | Bishkek 42.87 74.59 3, Osh 40.53 72.80 3
TJ Tajikistan 38.90 71.00 | Dushanbe 38.56 68.79 3
TM Turkmenistan 39.00 59.60 | Ashgabat 37.96 58.33 3
IL Israel 31.50 34.90 | Jerusalem 31.77 35.21 2, Tel Aviv 32.09 34.78 2, Haifa 32.79 34.99 3, Eilat 29.56 34.95 3
PS Palestine 31.90 35.20 | Ramallah 31.90 35.20 3, Bethlehem 31.70 35.20 3, Gaza 31.50 34.47 3
JO Jordan 31.20 36.50 | Amman 31.95 35.93 2, Petra 30.33 35.44 3, Aqaba 29.53 35.01 3
LB Lebanon 33.90 35.90 | Beirut 33.89 35.50 2, Byblos 34.12 35.65 3
SY Syria 35.00 38.50 | Damascus 33.51 36.29 2, Aleppo 36.20 37.16 3
IQ Iraq 33.00 43.70 | Baghdad 33.31 44.37 2, Erbil 36.19 44.01 3, Basra 30.51 47.78 3, Mosul 36.34 43.13 3, Najaf 32.03 44.34 3
IR Iran 32.00 53.70 | Tehran 35.69 51.39 1, Isfahan 32.65 51.67 2, Shiraz 29.59 52.58 2, Mashhad 36.30 59.60 3, Tabriz 38.08 46.29 3, Yazd 31.90 54.37 3, Kish 26.53 53.98 3
SA Saudi Arabia 24.00 45.00 | Riyadh 24.71 46.68 1, Jeddah 21.54 39.17 1, Mecca 21.39 39.86 2, Medina 24.47 39.61 2, Dammam 26.43 50.10 3, AlUla 26.61 37.92 3, Abha 18.22 42.51 3, Tabuk 28.38 36.57 3, Taif 21.27 40.42 3, Neom 28.00 35.20 3, Khobar 26.28 50.21 3, Yanbu 24.09 38.06 3, Buraidah 26.33 43.97 3, Hail 27.52 41.70 3, Jazan 16.89 42.55 3
AE United Arab Emirates 24.00 54.00 | Dubai 25.20 55.27 1, Abu Dhabi 24.47 54.37 2, Sharjah 25.35 55.40 3, Ras Al Khaimah 25.79 55.94 3, Al Ain 24.21 55.76 3, Fujairah 25.12 56.33 3
QA Qatar 25.30 51.20 | Doha 25.29 51.53 2
BH Bahrain 26.00 50.55 | Manama 26.23 50.59 3
KW Kuwait 29.30 47.50 | Kuwait City 29.38 47.99 2
OM Oman 21.50 55.90 | Muscat 23.59 58.41 2, Salalah 17.02 54.09 3, Nizwa 22.93 57.53 3
YE Yemen 15.50 47.60 | Sanaa 15.37 44.19 3, Aden 12.78 45.04 3
EG Egypt 26.80 30.00 | Cairo 30.04 31.24 1, Alexandria 31.20 29.92 2, Luxor 25.69 32.64 3, Aswan 24.09 32.90 3, Sharm El Sheikh 27.91 34.33 3, Hurghada 27.26 33.81 3, Giza 30.01 31.21 3, Port Said 31.27 32.30 3, Siwa 29.20 25.52 3, Dahab 28.50 34.51 3
LY Libya 27.00 17.00 | Tripoli 32.89 13.19 3, Benghazi 32.12 20.07 3
TN Tunisia 34.00 9.50 | Tunis 36.81 10.18 2, Sousse 35.83 10.64 3, Djerba 33.81 10.85 3
DZ Algeria 28.00 2.60 | Algiers 36.75 3.06 2, Oran 35.70 -0.64 3, Constantine 36.37 6.61 3, Tamanrasset 22.79 5.52 3
MA Morocco 31.80 -7.10 | Casablanca 33.57 -7.59 2, Marrakesh 31.63 -8.01 2, Rabat 34.02 -6.84 3, Fes 34.03 -5.00 3, Tangier 35.76 -5.80 3, Agadir 30.43 -9.60 3, Essaouira 31.51 -9.77 3, Chefchaouen 35.17 -5.27 3, Merzouga 31.10 -4.01 3, Ouarzazate 30.92 -6.89 3
MR Mauritania 20.30 -10.40 | Nouakchott 18.08 -15.98 3
SN Senegal 14.50 -14.50 | Dakar 14.72 -17.47 2, Saint-Louis 16.02 -16.50 3
GM Gambia 13.45 -15.30 | Banjul 13.45 -16.58 3
ML Mali 17.60 -3.50 | Bamako 12.64 -8.00 3, Timbuktu 16.77 -3.01 3
BF Burkina Faso 12.20 -1.60 | Ouagadougou 12.37 -1.52 3
NE Niger 17.60 8.10 | Niamey 13.51 2.11 3
GN Guinea 10.90 -10.90 | Conakry 9.64 -13.58 3
SL Sierra Leone 8.50 -11.80 | Freetown 8.48 -13.23 3
LR Liberia 6.40 -9.40 | Monrovia 6.30 -10.80 3
CI Ivory Coast 7.50 -5.50 | Abidjan 5.36 -4.01 2, Yamoussoukro 6.83 -5.29 3
GH Ghana 7.90 -1.00 | Accra 5.60 -0.19 2, Kumasi 6.69 -1.62 3
TG Togo 8.60 0.80 | Lome 6.13 1.22 3
BJ Benin 9.30 2.30 | Cotonou 6.37 2.39 3
NG Nigeria 9.10 8.70 | Lagos 6.52 3.38 1, Abuja 9.08 7.40 2, Kano 12.00 8.52 3, Ibadan 7.38 3.95 3, Port Harcourt 4.82 7.03 3
CM Cameroon 5.70 12.70 | Douala 4.05 9.77 3, Yaounde 3.85 11.50 3
TD Chad 15.40 18.70 | N'Djamena 12.13 15.05 3
CF Central African Republic 6.60 20.90 | Bangui 4.36 18.56 3
GA Gabon -0.80 11.60 | Libreville 0.42 9.47 3
CG Republic of the Congo -0.70 14.90 | Brazzaville -4.27 15.28 3
CD DR Congo -2.90 23.60 | Kinshasa -4.44 15.27 2, Lubumbashi -11.66 27.48 3, Goma -1.68 29.22 3
AO Angola -12.30 17.50 | Luanda -8.84 13.23 2, Lobito -12.35 13.54 3
ET Ethiopia 8.60 39.60 | Addis Ababa 9.03 38.74 2, Lalibela 12.03 39.04 3, Dire Dawa 9.59 41.87 3
ER Eritrea 15.40 38.90 | Asmara 15.32 38.93 3
DJ Djibouti 11.80 42.60 | Djibouti 11.59 43.15 3
SO Somalia 5.20 46.20 | Mogadishu 2.05 45.32 3
SD Sudan 15.50 30.20 | Khartoum 15.50 32.56 3
SS South Sudan 7.00 30.00 | Juba 4.85 31.58 3
KE Kenya 0.20 37.90 | Nairobi -1.29 36.82 2, Mombasa -4.04 39.67 3, Nakuru -0.30 36.07 3, Kisumu -0.09 34.77 3
UG Uganda 1.30 32.40 | Kampala 0.35 32.58 3, Entebbe 0.05 32.46 3
TZ Tanzania -6.30 34.90 | Dar es Salaam -6.79 39.21 2, Dodoma -6.16 35.75 3, Zanzibar -6.17 39.20 3, Arusha -3.37 36.68 3, Kilimanjaro -3.07 37.35 3
RW Rwanda -1.90 29.90 | Kigali -1.95 30.06 3
BI Burundi -3.40 29.90 | Gitega -3.43 29.92 3
MZ Mozambique -18.70 35.50 | Maputo -25.97 32.57 3, Beira -19.84 34.84 3
ZM Zambia -13.10 27.80 | Lusaka -15.39 28.32 3, Livingstone -17.84 25.85 3
ZW Zimbabwe -19.00 29.80 | Harare -17.83 31.05 3, Victoria Falls -17.93 25.83 3
MW Malawi -13.30 34.30 | Lilongwe -13.98 33.78 3
BW Botswana -22.30 24.70 | Gaborone -24.63 25.91 3, Maun -19.99 23.43 3
NA Namibia -22.60 17.10 | Windhoek -22.56 17.08 3, Swakopmund -22.68 14.53 3, Walvis Bay -22.96 14.51 3
ZA South Africa -29.00 25.00 | Johannesburg -26.20 28.05 1, Cape Town -33.92 18.42 1, Durban -29.86 31.02 2, Pretoria -25.75 28.19 2, Port Elizabeth -33.96 25.60 3, Bloemfontein -29.12 26.21 3, Stellenbosch -33.93 18.86 3, Knysna -34.04 23.05 3, Kimberley -28.74 24.77 3, Nelspruit -25.47 30.97 3
LS Lesotho -29.60 28.20 | Maseru -29.31 27.48 3
SZ Eswatini -26.50 31.50 | Mbabane -26.31 31.14 3
MG Madagascar -19.00 46.70 | Antananarivo -18.88 47.51 3, Nosy Be -13.33 48.26 3
MU Mauritius -20.30 57.60 | Port Louis -20.16 57.50 3
SC Seychelles -4.68 55.49 | Victoria -4.62 55.45 3
RE Reunion -21.12 55.54 | Saint-Denis -20.88 55.45 3
CV Cabo Verde 16.00 -24.00 | Praia 14.93 -23.51 3
ST Sao Tome and Principe 0.20 6.60 | Sao Tome 0.34 6.73 3
";

fn parse() -> Gazetteer {
    let mut countries = Vec::new();
    let mut cities = Vec::new();
    for line in DATA.lines() {
        let Some((head, list)) = line.split_once(" | ") else {
            continue;
        };
        let mut parts = head.rsplitn(3, ' ');
        let (lon, lat, who) = (parts.next(), parts.next(), parts.next());
        let (Some(lon), Some(lat), Some(who)) = (lon, lat, who) else {
            continue;
        };
        let Some((code, name)) = who.split_once(' ') else {
            continue;
        };
        let (lat, lon): (f32, f32) = (lat.parse().unwrap_or(0.0), lon.parse().unwrap_or(0.0));
        let country = countries.len() as u16;
        countries.push(Country { code, name, lat, lon });
        for item in list.split(", ") {
            let Some((rest, rank)) = item.rsplit_once(' ') else {
                continue;
            };
            let Some((rest, lon)) = rest.rsplit_once(' ') else {
                continue;
            };
            let Some((name, lat)) = rest.rsplit_once(' ') else {
                continue;
            };
            let (lat, lon): (f32, f32) = (lat.parse().unwrap_or(0.0), lon.parse().unwrap_or(0.0));
            let (wx, wy) = project(f64::from(lat), f64::from(lon));
            cities.push(City {
                name,
                country,
                lat,
                lon,
                wx,
                wy,
                rank: rank.parse().unwrap_or(3),
            });
        }
    }
    let cell_of = |c: &City| cell(c.lat, c.lon);
    let mut order: Vec<u16> = (0..cities.len() as u16).collect();
    order.sort_by_key(|&i| (cell_of(&cities[i as usize]), cities[i as usize].rank));
    let mut cells = vec![(0u32, 0u32); (COLS * ROWS) as usize];
    let mut at = 0usize;
    while at < order.len() {
        let c = cell_of(&cities[order[at] as usize]);
        let mut end = at;
        while end < order.len() && cell_of(&cities[order[end] as usize]) == c {
            end += 1;
        }
        cells[c] = (at as u32, end as u32);
        at = end;
    }
    Gazetteer {
        countries,
        cities,
        cells,
        order,
    }
}

/// The grid cell a point is in.
fn cell(lat: f32, lon: f32) -> usize {
    let (col, row) = cell_xy(lat, lon);
    (row * COLS + col) as usize
}

fn cell_xy(lat: f32, lon: f32) -> (i32, i32) {
    let col = (((lon + 180.0) / CELL).floor() as i32).rem_euclid(COLS);
    let row = (((90.0 - lat) / CELL).floor() as i32).clamp(0, ROWS - 1);
    (col, row)
}

/// The great-circle distance between two points, in km.
pub fn distance_km(lat_a: f32, lon_a: f32, lat_b: f32, lon_b: f32) -> f32 {
    let (la, lb) = (f64::from(lat_a).to_radians(), f64::from(lat_b).to_radians());
    let dlat = lb - la;
    let dlon = (f64::from(lon_b) - f64::from(lon_a)).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la.cos() * lb.cos() * (dlon / 2.0).sin().powi(2);
    (2.0 * 6371.0 * h.sqrt().min(1.0).asin()) as f32
}

static GAZETTEER: LazyLock<Gazetteer> = LazyLock::new(parse);

/// The one table, parsed the first time it is asked for.
pub fn gazetteer() -> &'static Gazetteer {
    &GAZETTEER
}

impl Gazetteer {
    /// Every city.
    pub fn cities(&self) -> &[City] {
        &self.cities
    }

    /// Every country.
    pub fn countries(&self) -> &[Country] {
        &self.countries
    }

    /// The country a city is in.
    pub fn country_of(&self, city: &City) -> &Country {
        &self.countries[usize::from(city.country).min(self.countries.len().saturating_sub(1))]
    }

    /// The city nearest to a point within `max_km`, searching outward ring by ring from the point's cell and
    /// stopping as soon as no cell farther out could hold a nearer one.
    pub fn nearest_within(&self, lat: f64, lon: f64, max_km: f32) -> Option<Nearest<'_>> {
        let (lat, lon) = (lat.clamp(-90.0, 90.0) as f32, lon.clamp(-180.0, 180.0) as f32);
        let (col, row) = cell_xy(lat, lon);
        let mut best: Option<Nearest> = None;
        // A cell's height is the same everywhere; its width shrinks toward the poles, so a ring's nearest edge
        // is at least `height * (ring - 1)` away in latitude terms alone.
        let step_km = CELL * 111.0;
        // Columns of the same ring are nearer the point the closer to a pole it is, so the ring's least
        // distance is scaled by the cosine of the highest latitude a cell could reach.
        let shrink = (lat.abs() + CELL).min(89.0).to_radians().cos().max(0.02);
        for ring in 0..=COLS / 2 {
            let floor_km = step_km * shrink * (ring as f32 - 1.0).max(0.0);
            let limit = best.map_or(max_km, |b| b.km.min(max_km));
            if floor_km > limit {
                break;
            }
            for r in (row - ring).max(0)..=(row + ring).min(ROWS - 1) {
                let edge_row = (r - row).abs() == ring;
                for dc in -ring..=ring {
                    if !edge_row && dc.abs() != ring {
                        continue;
                    }
                    let (from, to) = self.cells[(r * COLS + (col + dc).rem_euclid(COLS)) as usize];
                    for &i in &self.order[from as usize..to as usize] {
                        let city = &self.cities[usize::from(i)];
                        let km = distance_km(lat, lon, city.lat, city.lon);
                        if km <= max_km && best.is_none_or(|b| km < b.km) {
                            best = Some(Nearest { city, km });
                        }
                    }
                }
            }
        }
        best
    }

    /// The city nearest to a point, wherever it is on the map.
    pub fn nearest(&self, lat: f64, lon: f64) -> Option<Nearest<'_>> {
        self.nearest_within(lat, lon, 20_000.0)
    }
}

// ---- Naming places on the map -----------------------------------------------------------------------

/// A city's name, placed on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Label {
    /// Where the city is, in the map's box.
    pub sx: f32,
    pub sy: f32,
    pub name: &'static str,
    pub rank: u8,
}

impl Label {
    /// The box the name takes: `[left, top, right, bottom]`, to the right of the city's dot.
    pub fn bounds(&self) -> [f32; 4] {
        let width = self.name.chars().count() as f32 * LETTER_PX + 4.0;
        [
            self.sx + 5.0,
            self.sy - LABEL_H * 0.5,
            self.sx + 5.0 + width,
            self.sy + LABEL_H * 0.5,
        ]
    }
}

/// The farthest a city can be and still be what a photo was taken "near".
pub const NEAR_KM: f32 = 300.0;

/// What a label's text is taken to measure: about what an 11 px label takes per letter, and its height.
const LETTER_PX: f32 = 6.2;
const LABEL_H: f32 = 14.0;
/// No more labels than this in one view, however far in the map is.
const MOST_LABELS: usize = 60;

/// The smallest cities named at a zoom: none while the map shows the world, world cities from 2.5, large ones
/// from 7, the rest from 18.
fn rank_shown(zoom: f64) -> u8 {
    if zoom >= 18.0 {
        3
    } else if zoom >= 7.0 {
        2
    } else if zoom >= 2.5 {
        1
    } else {
        0
    }
}

impl Gazetteer {
    /// The cities to name on a map seen by `cam` in a `width` x `height` box: the ones the zoom is deep enough
    /// for, biggest first, none whose name would sit on another's. Worked out once per camera, not per frame.
    pub fn labels(&self, cam: &MapCamera, width: f32, height: f32) -> Vec<Label> {
        let shown = rank_shown(cam.zoom);
        if shown == 0 || width <= 1.0 || height <= 1.0 {
            return Vec::new();
        }
        let mut found: Vec<(u8, u16, f32, f32)> = Vec::new();
        for (i, city) in self.cities.iter().enumerate() {
            if city.rank > shown {
                continue;
            }
            let (sx, sy) = cam.to_screen(city.wx, city.wy, width, height);
            if sx >= -4.0 && sy >= -4.0 && sx <= width + 4.0 && sy <= height + 4.0 {
                found.push((city.rank, i as u16, sx, sy));
            }
        }
        found.sort_by_key(|&(rank, i, ..)| (rank, i));
        let mut labels: Vec<Label> = Vec::new();
        let mut boxes: Vec<[f32; 4]> = Vec::new();
        for (rank, i, sx, sy) in found {
            let label = Label {
                sx,
                sy,
                name: self.cities[usize::from(i)].name,
                rank,
            };
            let at = label.bounds();
            let clear = boxes
                .iter()
                .all(|b| at[2] <= b[0] || at[0] >= b[2] || at[3] <= b[1] || at[1] >= b[3]);
            if clear {
                boxes.push(at);
                labels.push(label);
                if labels.len() >= MOST_LABELS {
                    break;
                }
            }
        }
        labels
    }

    /// Where a point is, in a city's name: "Tokyo", with the country; None when no city is within
    /// [`NEAR_KM`].
    pub fn near(&self, lat: f64, lon: f64) -> Option<(&'static str, &'static str)> {
        let found = self.nearest_within(lat, lon, NEAR_KM)?;
        Some((found.city.name, self.country_of(found.city).name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_parses_whole() {
        let g = gazetteer();
        assert!(g.cities().len() > 450, "{} cities", g.cities().len());
        assert!(g.countries().len() > 120);
        assert!(DATA.len() < 60_000, "{} bytes", DATA.len());
        for c in g.cities() {
            assert!(
                (-90.0..=90.0).contains(&c.lat) && (-180.0..=180.0).contains(&c.lon),
                "{}",
                c.name
            );
            assert!((1..=3).contains(&c.rank), "{}", c.name);
            assert!(!c.name.is_empty() && !c.name.contains(','), "{}", c.name);
        }
        for c in g.countries() {
            assert!(!c.name.is_empty() && c.code.len() == 2, "{}", c.name);
        }
        assert_eq!(g.country_of(&g.cities()[0]).name, "Japan");
    }

    #[test]
    fn known_places_resolve_to_their_cities() {
        let g = gazetteer();
        for (lat, lon, name, country) in [
            (35.676, 139.650, "Tokyo", "Japan"),
            (24.713, 46.675, "Riyadh", "Saudi Arabia"),
            (30.044, 31.236, "Cairo", "Egypt"),
            (40.713, -74.006, "New York", "United States"),
            (-23.551, -46.633, "São Paulo", "Brazil"),
            (-33.869, 151.209, "Sydney", "Australia"),
            (64.146, -21.942, "Reykjavík", "Iceland"),
        ] {
            let found = g.nearest(lat, lon).unwrap_or_else(|| panic!("nothing near {name}"));
            assert_eq!(found.city.name, name);
            assert_eq!(g.country_of(found.city).name, country);
            assert!(found.km < 5.0, "{name} {} km", found.km);
        }
    }

    #[test]
    fn a_suburb_belongs_to_the_nearest_city_and_the_ocean_to_none() {
        let g = gazetteer();
        // Versailles is 17 km from Paris.
        assert_eq!(g.nearest_within(48.80, 2.13, 300.0).map(|n| n.city.name), Some("Paris"));
        // The middle of the Pacific has nothing within 300 km.
        assert!(g.nearest_within(-40.0, -130.0, 300.0).is_none());
        // Across the antimeridian: Suva is the nearest to a point just east of it.
        assert_eq!(
            g.nearest_within(-18.1, -179.9, 300.0).map(|n| n.city.name),
            Some("Suva")
        );
    }

    #[test]
    fn the_grid_agrees_with_a_scan_of_every_city() {
        let g = gazetteer();
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64) / ((1u64 << 31) as f64)
        };
        for _ in 0..400 {
            let (lat, lon) = (next() * 170.0 - 85.0, next() * 360.0 - 180.0);
            let slow = g
                .cities()
                .iter()
                .map(|c| (c, distance_km(lat as f32, lon as f32, c.lat, c.lon)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(c, _)| c.name);
            assert_eq!(g.nearest(lat, lon).map(|n| n.city.name), slow, "at {lat}, {lon}");
        }
    }

    #[test]
    fn names_appear_when_zoomed_in_and_never_sit_on_each_other() {
        let g = gazetteer();
        // The whole world: no names.
        assert!(g.labels(&MapCamera::default(), 1200.0, 700.0).is_empty());
        // Over Japan and Korea at the scale of large cities.
        let (wx, wy) = project(35.0, 136.0);
        let cam = MapCamera {
            cx: wx,
            cy: wy,
            zoom: 9.0,
        };
        let labels = g.labels(&cam, 1200.0, 700.0);
        let names: Vec<&str> = labels.iter().map(|l| l.name).collect();
        assert!(
            names.contains(&"Tokyo") && names.contains(&"Osaka") && names.contains(&"Seoul"),
            "{names:?}"
        );
        assert!(!names.contains(&"Paris"), "off the map");
        assert!(
            labels.iter().all(|l| l.rank <= 2),
            "only world cities and large ones at this zoom"
        );
        for (i, a) in labels.iter().enumerate() {
            let at = a.bounds();
            for b in &labels[i + 1..] {
                let other = b.bounds();
                let apart = at[2] <= other[0] || at[0] >= other[2] || at[3] <= other[1] || at[1] >= other[3];
                assert!(apart, "{} and {} overlap", a.name, b.name);
            }
        }
        // Zoomed in further, smaller cities are named too, still without overlapping.
        let near = MapCamera { zoom: 24.0, ..cam };
        let more = g.labels(&near, 1200.0, 700.0);
        assert!(
            more.iter().any(|l| l.rank == 3),
            "{:?}",
            more.iter().map(|l| l.name).collect::<Vec<_>>()
        );
        // No box, no names.
        assert!(g.labels(&cam, 0.0, 0.0).is_empty());
    }

    #[test]
    fn where_a_cluster_is_reads_as_a_city_and_its_country_or_as_nothing() {
        let g = gazetteer();
        assert_eq!(g.near(35.676, 139.650), Some(("Tokyo", "Japan")));
        assert_eq!(g.near(48.80, 2.13), Some(("Paris", "France")));
        // The middle of the Pacific, and the Sahara far from any city in the table: nothing near.
        assert_eq!(g.near(-40.0, -130.0), None);
    }
}
