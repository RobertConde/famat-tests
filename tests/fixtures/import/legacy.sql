-- A synthetic stand-in for the old site's dump (doralmao_famat.sql), in the
-- same mysqldump format. Only test_id and documents matter to the import.

SET SQL_MODE = "NO_AUTO_VALUE_ON_ZERO";

CREATE TABLE `test_bank` (
  `test_id` int(11) NOT NULL,
  `category` varchar(255) NOT NULL,
  `level` varchar(255) NOT NULL,
  `style` varchar(255) NOT NULL,
  `year` int(11) NOT NULL,
  `host` varchar(255) NOT NULL,
  `calculator` varchar(1) NOT NULL,
  `documents` longtext NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO `test_bank` (`test_id`, `category`, `level`, `style`, `year`, `host`, `calculator`, `documents`) VALUES
(1, 'Invitationals - January', 'Algebra I', '(Individual)', 1999, 'Berkeley Prep', 'N', '[{\"tid\": 766, \"doc_title\": \"Test\"}, {\"tid\": 767, \"doc_title\": \"Solutions\"}]'),
(2, 'Invitationals - January', 'Algebra I', '(Team)', 1999, 'Berkeley Prep', 'N', '[{\"tid\": 768, \"doc_title\": \"Solutions\"}, {\"tid\": 769, \"doc_title\": \"Solutions\"}]'),
(3, 'Invitationals - January', 'Algebra II', '(Individual)', 1999, 'Berkeley Prep', 'N', '[{\"tid\": 770, \"doc_title\": \"Solutions\"}, {\"tid\": 771, \"doc_title\": \"Test\"}]'),
(1500, 'Nationals', 'Theta', '(Individual)', 1989, 'MAO', 'N', '[{\"tid\": 1160, \"doc_title\": \"Test\"}, {\"tid\": 1162, \"doc_title\": \"Answers\"}]'),
(1501, 'Nationals', 'Precalculus/Alpha', '(Individual)', 1989, 'MAO', 'N', '[{\"tid\": 1161, \"doc_title\": \"Test\"}]'),
(1888, '?', '?', '?', 0, '', 'N', '[{\"tid\": 5617, \"doc_title\": \"\"}, {\"tid\": 5618, \"doc_title\": \"\"}]'),
(1950, 'Nationals', 'Theta', 'Sequences and Series', 2004, 'MAO', 'N', '[{\"tid\": 5585, \"doc_title\": \"Test\"}]');
INSERT INTO `test_bank` (`test_id`, `category`, `level`, `style`, `year`, `host`, `calculator`, `documents`) VALUES
(2000, 'Nationals', 'Calculus/Mu', 'Applications', 2008, 'MAO', 'N', '[{\"tid\": 5147, \"doc_title\": \"Answers\"}, {\"tid\": 5149, \"doc_title\": \"Test\"}]'),
(2100, 'States', 'Calculus/Mu', 'Applications', 2008, 'FAMAT', 'N', '[{\"tid\": 4925, \"doc_title\": \"Test\"}]'),
(2200, 'States', 'Calculus/Mu', 'Applications', 2000, 'FAMAT', 'N', '[{\"tid\": 4811, \"doc_title\": \"Test\"}, {\"tid\": 4833, \"doc_title\": \"Test\"}]'),
(2300, 'States', 'Calculus/Mu', 'Applications', 1995, 'FAMAT', 'N', '[{\"tid\": 3907, \"doc_title\": \"Solutions\"}]'),
(2301, 'States', 'Calculus/Mu', 'Applications', 1996, 'FAMAT', 'N', '[{\"tid\": 3909, \"doc_title\": \"Solutions\"}, {\"tid\": 3910, \"doc_title\": \"Test\"}]'),
(2302, 'States', 'Calculus/Mu', 'Applications', 1997, 'FAMAT', 'N', '[{\"tid\": 3911, \"doc_title\": \"Solutions\"}]'),
(2400, 'States', 'Precalculus/Alpha', 'Applications', 2005, 'FAMAT', 'N', '[{\"tid\": 5700, \"doc_title\": \"Test\"}]'),
(2500, 'Washington State', 'Open', 'Relays', 2007, '', 'N', '[{\"tid\": 6051, \"doc_title\": \"Answers\"}, {\"tid\": 6052, \"doc_title\": \"Test\"}]'),
(3000, 'Regionals - February', 'Algebra II', '(Team)', 2012, 'FAMAT', 'N', '[{\"tid\": 7631, \"doc_title\": \"Test\"}, {\"tid\": 9000, \"doc_title\": \"Test\"}]'),
(3500, 'Nationals', 'Open', 'State Bowl (Nationals)', 1997, 'MAO', 'N', '[{\"tid\": 1600, \"doc_title\": \"Test\"}]'),
(3600, 'Regionals - January', 'Geometry', '(Individual)', 2021, 'FAMAT', 'N', '[{\"tid\": 10005, \"doc_title\": \"Test\"}]'),
(3650, 'States', 'Calculus/Mu', '(Team)', 2019, 'FAMAT', 'N', '[{\"tid\": 9800, \"doc_title\": \"Test\"}, {\"tid\": 9801, \"doc_title\": \"Answers\"}]'),
(3700, 'Mail-in', 'Theta', '(Individual)', 2010, 'FAMAT', 'N', '[{\"tid\": 9900, \"doc_title\": \"Test\"}]'),
(3706, '?', 'Calculus/Mu', 'Integration', 0, 'MAO', 'N', '[{\"tid\": 2118, \"doc_title\": \"Test\"}]');
