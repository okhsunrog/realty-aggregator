-- The feed encodes these values both as JSON numbers and as strings.
UPDATE listings SET
    rooms = COALESCE(rooms, CASE WHEN params->>'Количество комнат' ~ '^[0-9]{1,3}$' THEN (params->>'Количество комнат')::integer END),
    floor = COALESCE(floor, CASE WHEN params->>'Этаж' ~ '^-?[0-9]{1,3}$' THEN (params->>'Этаж')::integer END),
    floors_total = COALESCE(floors_total, CASE WHEN params->>'Этажей в доме' ~ '^[0-9]{1,3}$' THEN (params->>'Этажей в доме')::integer END),
    area = COALESCE(area, CASE WHEN params->>'Площадь' ~ '^[0-9]{1,4}([.,][0-9]{1,3})?$' THEN replace(params->>'Площадь', ',', '.')::double precision END);
